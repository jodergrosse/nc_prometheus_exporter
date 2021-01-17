#![feature(proc_macro_hygiene, decl_macro)]
#[macro_use] extern crate rocket;

use confy;
use serde_json;
use serde_json::Value;
use std::env::current_exe;
use std::fs::File;
use std::path::PathBuf;
use std::io::prelude::*;
use log::{error, debug, warn};
use std::str::FromStr;
use fern;

mod lib;

/// # The nextcloud prometheus exporter
///   * loads the xml status page exposed by a nextcloud instance [[1]](lib::load_status_page)
///   * converts the xml output into prometheus metrics [[1]](lib::xml_to_prometheus) [[2]](nc_metric_to_number)
///   * exposes them using a rocket webserver [[1]](lib::index)
fn main() {
    setup_logger().expect("Logger setup.");

    let mut cfg_path = current_exe().unwrap();
    cfg_path.pop();
    cfg_path = cfg_path.join("nc_prometheus_exporter.conf");
    if !cfg_path.exists() {
        panic!("No config found in {:?}.\nNextcloud credentials are required for the exporter to work.", cfg_path);
    }
    let path = cfg_path.as_path().display().to_string();

    let cfg: Result<lib::Config, confy::ConfyError> = confy::load_path(cfg_path);
    let config = match cfg {
        Ok(config) => {
            if config.nc_password.is_empty() || config.nc_user.is_empty() {
                warn!("Nextcloud user credentials are empty.");
            }
            if config.nc_url.is_empty() {
                warn!("Nextcloud status page URL config ist empty.");
            }
            if config.nc_password.is_empty() || config.nc_user.is_empty() || config.nc_url.is_empty(){
                warn!("Consider updating the configuration ({}).", path);
            }
            config
        },
        Err(e) => {
            error!("Error while loading config: {}", e);
            lib::Config::default()
        },
    };
    debug!("Config loaded {}", config);

    let replace_config = load_replace_config(&config.nc_replacement_config);
    debug!("Replace config loaded {}", replace_config);

    rocket::ignite()
        .manage(config)
        .manage(replace_config)
        .manage(lib::RequestCounter::new())
        .mount("/", routes![lib::index])
        .launch();
}


fn load_replace_config(file_path: &str) -> Value {
    // loading replace config if in config
    let mut rep_cfg_path = PathBuf::from(file_path);
    if rep_cfg_path.is_relative() {
        rep_cfg_path = current_exe().unwrap();
        rep_cfg_path.pop();
        rep_cfg_path = rep_cfg_path.join(file_path);
    }

    debug!("Reading replace config from: {:?}", rep_cfg_path);
    if rep_cfg_path.exists() {
        let mut file = File::open(&rep_cfg_path)
            .expect(&format!("Couldn't open replace configuration ({:?})", &rep_cfg_path));
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .expect(&format!("The replace configuration could not be read! ({:?})", &rep_cfg_path));
        let config = serde_json::from_str(&contents);

        return match config {
            Ok(cfg) => cfg,
            Err(_e) => get_empty_config(),
        };
    }

    error!("Replacement config file doesnt exist: {}", file_path);
    get_empty_config()
}

fn get_empty_config() -> Value {
    serde_json::from_str("{\"names\": {}, \"values\": {}}").expect("Empty replace config.")
}

/// Setup logger which logs to file and stdout.
/// Log level can be set with the environment variable `RUST_LOG`.
fn setup_logger() -> Result<(), fern::InitError> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(
            log::LevelFilter::from_str(
                &std::env::var("RUST_LOG").unwrap_or("info".to_string())
            )
            .unwrap_or(log::LevelFilter::Info)
        )
        .chain(std::io::stdout())
        .apply()?;
    Ok(())
}
