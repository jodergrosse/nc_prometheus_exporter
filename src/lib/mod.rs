use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Serialize, Deserialize};
use serde_json::Value;

use std::time::Instant;
use std::fmt;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use hex;
use log::{warn, error, debug};

use rocket::State;

use reqwest;
use reqwest::StatusCode;
use reqwest::blocking::Client;


#[derive(Debug)]
pub struct RequestCounter {
    start: AtomicUsize,
    end: AtomicUsize,
}

impl RequestCounter {
    pub fn new() -> RequestCounter {
        RequestCounter{
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0)
        }
    }

    fn count_start(&self) {
        self.start.fetch_add(1, Ordering::Relaxed);
    }

    fn count_end(&self) {
        self.end.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Deserialize, Serialize)]
pub struct Config {
    pub nc_url: String,
    pub nc_user: String,
    pub nc_password: String,
    pub nc_replacement_config: String,
}

impl ::std::default::Default for Config {
    fn default() -> Self { 
        Self { 
            nc_url: "".to_string(),
            nc_user: "".to_string(),
            nc_password: "".to_string(),
            nc_replacement_config: "replacements.json".to_string(),
        } 
    }
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "nce config:\nnc_url = \"{}\"\nnc_user = \"{}\"\nnc_password = \"{}\"\nnc_replacement_config = \"{}\"", 
            self.nc_url, 
            self.nc_user,
            if self.nc_password.len() > 0 {
                "*****"
            } else {
                ""
            },
            self.nc_replacement_config,
        )
    }
}

#[get("/")]
pub fn index(cfg: State<Config>, replace_cfg: State<Value>, req_counter: State<RequestCounter>) -> Option<String> {
    let timer = Instant::now();
    req_counter.count_start();

    let xml = match load_status_page(
        &cfg.nc_url, &cfg.nc_user, &cfg.nc_password
    ) {
        Some(text) => text,
        None => return None,
    };
    let dur_load = timer.elapsed().as_secs_f32();

    let prom_str = xml_to_prometheus(&xml, replace_cfg.inner());
    let dur_total = timer.elapsed().as_secs_f32();
    let dur_parse = dur_total - dur_load;
    
    req_counter.count_end();
    Some(format!(
        "{}\n{} {}\n{} {}\n{} {}\n{} {}\n{} {}\n{}\n{}\n{}",
        "# exporter duration",
        "rust_nce_parse_duration", dur_parse,
        "rust_nce_load_duration", dur_load,
        "rust_nce_total_duration", dur_total,
        "rust_nce_request_start_count", req_counter.start.load(Ordering::Relaxed),
        "rust_nce_request_end_count", req_counter.end.load(Ordering::Relaxed),
        "# nextcloud metrics",
        "ocs_meta_up 1",
        prom_str
    ))
}

/// Loads the nextcloud status page using nc admin user credentials
pub fn load_status_page(url: &str, user: &str, password: &str) -> Option<String> {
    let client = Client::new();
    let response = client.get(url)
            .basic_auth(user, Some(password))
            .send();

    debug!("Response {:?}", response);
    match response {
        Ok(response) => {
            let status = response.status();
            match status {
                StatusCode::OK => {
                    let text = response.text();
                    match text {
                        Ok(text) => Some(text),
                        Err(e) => {
                            warn!("There was a problem loading the result: : {}", e);
                            None
                        },
                    }
                },
                _ => {
                    warn!("Status code is not 200: {}", status);
                    None
                },
            }
        },
        Err(e) => {
            error!("Request of Nextcloud status failed (url=\"{}\"): {}", url, e);
            None
        },
    }
}

/// Converts the xml status page into prometheus compatible metrics.
/// Some parts of the status page contain string values.
/// The function [`nc_metric_to_number`](nc_metric_to_number) is used to either ignore
/// or convert them into a numeric value.
/// 
/// Also creates and stores part of a [hash](https://github.com/prometheus/alertmanager/issues/596)
/// of the metric names. This is helpful to see if the status page structure was changed, since
/// that may require adjustments to this exporter or prometheus alerts.
/// 
/// * `xml` - the nextcloud xml status page
pub fn xml_to_prometheus(xml: &str, replace_cfg: &Value) -> String{
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);

    let mut txt = Vec::new();
    let mut metric_names = HashMap::new();
    let mut buf = Vec::new();
    let mut parent_stack = Vec::new();

    loop {
        match reader.read_event(&mut buf) {
            Ok(Event::Start(ref e)) => {
                parent_stack.push(String::from_utf8(e.name().to_vec()).unwrap());
            },
            Ok(Event::Text(e)) => {
                let raw_text = &e.unescape_and_decode(&reader).unwrap();
                let mut metric_name = xml_path_to_metric_name(&parent_stack);

                // unescape and decode the text event using the reader encoding
                let metric = nc_metric_to_number(raw_text, &replace_cfg["values"]);

                match metric {
                    Ok(val) => {
                        let name_count = metric_names.entry(metric_name.clone())
                                            .or_insert(0);
                        *name_count += 1;

                        if *name_count > 1 {
                            metric_name = format!("{}{}", metric_name, name_count);
                        }

                        txt.push(
                            format!(
                                "{} {}",
                                metric_name,
                                val
                            )
                        );
                    },
                    Err(invalid_val) => {
                        debug!("IGNORED METRIC: {} {}", metric_name, invalid_val);
                        ()
                    }, 
                };
            },
            Ok(Event::End(ref _e)) => {
                parent_stack.pop();
            },
            Ok(Event::Eof) => break, // exits the loop when reaching end of file
            Err(e) => {
                log::warn!("Make sure you configured the right url!");
                log::error!("Error while parsing xml at position {}: {:?}", reader.buffer_position(), e);
                ()
            },
            _ => (),
        }

        buf.clear();
    }

    // calculate a hash of a sorted list of names to make changes visible
    let mut all_names = metric_names.keys().collect::<Vec<&String>>();
    all_names.sort();
    let mut names_text = "".to_string();
    for met_name in all_names {
        names_text.push_str(&format!("{}\n", met_name))
    }
    let hash = &md5::compute(&names_text);
    let md5_metric = i32::from_str_radix(&hex::encode(&hash[0..3]), 16).unwrap();

    txt.push("# nc_metric_names_hash: first digits of a hash of all extracted metric names".to_string());
    txt.push("# this number indicates change of names or change of number of metrics".to_string());
    txt.push(format!("{} {}", "nc_metric_names_hash", md5_metric));

    txt.join("\n")
}

/// Converts string values to numeric values and returns them
/// as string. 
/// Replaces strings with numbers as configured in the 
/// replacement config. (yes -> 1, no -> 0 and the like)
fn nc_metric_to_number(value: &str, replace_dict: &Value) -> Result<String, String> {
    let metric = &value.trim().parse::<f64>();
    match metric {
        Ok(val) => Ok(format!("{}", val)),
        Err(e) => {
            match replace_dict.get(value.trim()) {
                Some(value) => Ok(value.to_string()),
                None => Err(format!("Error when trying to convert {}\n{:?}", value, e)),
            }
        },
    }
}

/// Usually joins parts of the xml path with underscore
/// If a replacement is defined
fn xml_path_to_metric_name (path: &[String]) -> String {
    let name = path.join("_").replace(".", "_");
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::*;

    #[test]
    fn test_nc_metric_to_number() {
        let empty_replace_cfg = get_empty_config();

        assert_eq!(
            nc_metric_to_number("12", &empty_replace_cfg),
            Ok("12".to_string())
        )
    }
    #[test]
    fn test_nc_metric_to_number1() {
        let empty_replace_cfg = get_empty_config();
        assert_eq!(
            nc_metric_to_number("   12", &empty_replace_cfg),
            Ok("12".to_string())
        )
    }
    #[test]
    fn test_nc_metric_to_number2() {
        let empty_replace_cfg = get_empty_config();
        assert_eq!(
            nc_metric_to_number("Ok", &empty_replace_cfg),
            Err("Error when trying to convert Ok\nParseFloatError { kind: Invalid }".to_string())
        )
    }
    #[test]
    fn test_nc_metric_to_number_replace() {
        let replace_cfg: Value = serde_json::from_str(r#"
        {
            "values" : {
                "ok": 1
            }
        }"#).expect("config");

        assert_eq!(
            nc_metric_to_number("Foo", &replace_cfg["values"]),
            Err("Error when trying to convert Foo\nParseFloatError { kind: Invalid }".to_string())
        )
    }
    #[test]
    fn test_nc_metric_to_number_replace2() {
        let replace_cfg: Value = serde_json::from_str(r#"
        {
            "values" : {
                "ok": 1
            }
        }"#).expect("config");

        assert_eq!(
            nc_metric_to_number("ok", &replace_cfg["values"]),
            Ok("1".to_string())
        )
    }

    #[test]
    fn tets_path_to_name() {
        assert_eq!(
            xml_path_to_metric_name(&vec!["test".to_string(),"path".to_string(),"example".to_string()]),
            "test_path_example".to_string()
        )
    }

    #[test]
    /// xml to prometheus with xml snippet and empty replace config
    fn test_xml_to_prometheus() {
        let xml = r#"<storage>
            <num_users>42</num_users>
            <num_files>149545</num_files>
            <num_storages>66</num_storages>
            <num_storages_local>1</num_storages_local>
            <num_storages_home>65</num_storages_home>
            <num_storages_other>0</num_storages_other>
        </storage>"#.to_string();

        let empty_replace_cfg = get_empty_config();

        let result = 
r#"storage_num_users 42
storage_num_files 149545
storage_num_storages 66
storage_num_storages_local 1
storage_num_storages_home 65
storage_num_storages_other 0
# nc_metric_names_hash: first digits of a hash of all extracted metric names
# this number indicates change of names or change of number of metrics
nc_metric_names_hash 16071814"#.to_string();

        assert_eq!(xml_to_prometheus(&xml, &empty_replace_cfg), result)
    }

    #[test]
    fn test_xml_to_prometheus_with_config() {
        let xml = r#"<storage>
            <num_users>42</num_users>
            <num_files>149545</num_files>
            <num_storages>66</num_storages>
            <num_storages_local>1</num_storages_local>
            <num_storages_home>65</num_storages_home>
            <num_storages_other>0</num_storages_other>
        </storage>"#.to_string();

        let replace_cfg = serde_json::from_str(r#"
        {
            "values" : {
                "ok": 1,
                "yes": 1,
                "OK": 1,
                "none": 0,
                "no": 0
            }
        }"#).expect("config");

        let result = 
r#"storage_num_users 42
storage_num_files 149545
storage_num_storages 66
storage_num_storages_local 1
storage_num_storages_home 65
storage_num_storages_other 0
# nc_metric_names_hash: first digits of a hash of all extracted metric names
# this number indicates change of names or change of number of metrics
nc_metric_names_hash 16071814"#.to_string();

        assert_eq!(xml_to_prometheus(&xml, &replace_cfg), result)
    }

    #[test]
    fn test_xml_to_prometheus_with_replacements() {
        let xml = r#"<storage>
            <num_users>42</num_users>
            <num_files>OK</num_files>
            <num_storages>none</num_storages>
            <num_storages_local>yes</num_storages_local>
            <num_storages_home>ok</num_storages_home>
            <num_storages_other>no</num_storages_other>
        </storage>"#.to_string();

        let replace_cfg = serde_json::from_str(r#"
        {
            "values" : {
                "ok": 1,
                "yes": 1,
                "OK": 1,
                "none": 0,
                "no": 0
            }
        }"#).expect("config");

        let result = 
r#"storage_num_users 42
storage_num_files 1
storage_num_storages 0
storage_num_storages_local 1
storage_num_storages_home 1
storage_num_storages_other 0
# nc_metric_names_hash: first digits of a hash of all extracted metric names
# this number indicates change of names or change of number of metrics
nc_metric_names_hash 16071814"#.to_string();

        assert_eq!(xml_to_prometheus(&xml, &replace_cfg), result)
    }

    #[test]
    fn test_xml_to_prometheus_with_duplicate_names() {
        let xml = r#"<storage>
            <num_users>42</num_users>
            <num_users>42</num_users>
            <num_users>42</num_users>
            <num_files>OK</num_files>
            <num_storages>none</num_storages>
        </storage>"#.to_string();

        let replace_cfg = serde_json::from_str(r#"
        {
            "values" : {
                "ok": 1,
                "yes": 1,
                "OK": 1,
                "none": 0,
                "no": 0
            }
        }"#).expect("config");

        let result = 
r#"storage_num_users 42
storage_num_users2 42
storage_num_users3 42
storage_num_files 1
storage_num_storages 0
# nc_metric_names_hash: first digits of a hash of all extracted metric names
# this number indicates change of names or change of number of metrics
nc_metric_names_hash 16217419"#.to_string();

        assert_eq!(xml_to_prometheus(&xml, &replace_cfg), result)
    }
}