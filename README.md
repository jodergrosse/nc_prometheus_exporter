# nc prometheus exporter
Nc prometheus exporter is a little webserver, which translates the nextcloud status xml
to a prometheus compatible format.

## Usage
```shell
NCE_CONF=nc_prometheus_exporter.conf nc_prometheus_exporter
```

### Environment
| key                    | description                | default |
|:-----------------------|:---------------------------|:--------|
| NCE_CONF               | Path to configuration file | /etc/ncexporter/nc_prometheus_exporter.conf |
| NCE_PORT               | tcp port                   | 8000    |
| RUST_LOG               | Rust log level             | warn    |


### Nextcloud endpoint configuration
| key                    | description                       |
|:-----------------------|:----------------------------------|
| nc_url                 | Url of a nextcloud instance       |
| nc_user                | Username of an admin user         |
| nc_password            | App password of the admin user    |
| nc_replacement_config  | Path to replacement config (json) |

#### Example configuration
```conf
nc_url = 'https://your.domain.tld/ocs/v2.php/apps/serverinfo/api/v1/info'
nc_user = 'example_user'
nc_password = 'example_pw'
nc_replacement_config = 'replacements.json'
```
> Hint: Create and use a device password [(Nextcloud documentation)](https://docs.nextcloud.com/server/latest/user_manual/en/session_management.html#managing-devices)

### Replacement configuration
The json replacement configuration defines how to replace values during translations.
Some Nextcloud info values are not numeric, and therefore not compatible with
prometheus.
Values will be ignored if not numeric and no string to numeric replacement
pair is defined.

Ignored metrics will be displayed via logging output on log level debug.

#### Example configuration

```json
{
    "values" : {
        "ok": 1,
        "yes": 1,
        "OK": 1,
        "none": 0,
        "no": 0
    }
}
```

## Translation process

After fetching the xml status information from a nextcloud instance,
each entry is processed by the exporter.
The xml node names will be combined into a prometheus time series name.

Numeric values will be used as prometheus metric. Non numeric values
will be replaced with a numeric value if configured, or otherwise
dropped.

Additional to nextcloud metrics a few metrics regarding the export will be
added.

### Additional exporter metrics

| name                         | description |
|:-----------------------------|:------------|
| rust_nce_parse_duration      | Time spent parsing the status xml  |
| rust_nce_load_duration       | Duration of loading the status xml from the configured nextcloud instance |
| rust_nce_total_duration      | Combined duration of load, parsing and translation |
| rust_nce_request_start_count | Count of exporter requests |
| rust_nce_request_end_count   | Count of answered exporter requests |
| nc_metric_names_hash         | Fisrst digits of a md5 hash based on a sorted list of metric names. A change indicates fewer, swapped or additional metrics. |