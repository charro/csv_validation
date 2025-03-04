use std::fs;
use std::fs::File;
use std::io::{BufReader, Read};
use std::iter::zip;
use std::path::Path;
use csv::Reader;
use flate2::bufread::GzDecoder;
use log::info;
use pyo3::prelude::*;
use regex::Regex;
use yaml_rust2::YamlLoader;

struct ColumnValidation {
    column_name: String,
    validation: Regex
}

/// Validate that CSV file complies with validations definition
#[pyfunction]
fn validate(path: &str, definition_path: &str) -> PyResult<bool> {
    info!("Validating file {} against definition {}", path, definition_path);

    let validations = get_validations(definition_path);

    // Build the CSV reader
    let mut rdr = get_reader_from(path);

    // First validation: Ensure column names and order are exactly as expected
    if validate_column_names(&mut rdr, &validations) {
        info!("Columns names and order are correct");
    }
    else {
        info!("Expected columns != Real columns");
        return Ok(false);
    }

    for result in rdr.records() {
        let record = result.unwrap();
        for next_column in zip(record.iter(), validations.iter()) {
            let value = next_column.0;
            let column_name = &next_column.1.column_name;
            let re = &next_column.1.validation;
            if !re.is_match(value) {
                info!("Column [{}] => Value {:?} doesn't match regex {:?}", column_name, value, re);
                return Ok(false);
            }
        }
    }

    info!("File matches the validations");
    Ok(true)
}

/// Infers the file compression type and returns the corresponding buffered reader
fn get_reader_from(path: &str) -> Reader<Box<dyn Read>> {
    let buf_reader = BufReader::new(File::open(Path::new(path)).unwrap());
    if is_gzip_file(path) {
        info!("File is gzipped");
        let read_capacity = 10 * 1024_usize.pow(2);
        let reader = BufReader::with_capacity(read_capacity, GzDecoder::new(buf_reader));
        Reader::from_reader(Box::new(reader))
    }
    else {
        Reader::from_reader(Box::new(buf_reader))
    }
}

fn is_gzip_file(path: &str) -> bool {
    let mut bytes = [0u8; 2];
    File::open(Path::new(path)).unwrap().read_exact(&mut bytes).unwrap();
    bytes[0] == 0x1f && bytes[1] == 0x8b
}

fn get_validations(definition_path: &str) -> Vec<ColumnValidation> {
    // Read the YAML definition with the validations
    let config =
        YamlLoader::load_from_str(fs::read_to_string(definition_path).unwrap().as_str()).unwrap();
    // Get the column names list and each associated validation
    let columns = &config[0]["columns"];
    let mut column_names = vec![];
    let mut validations = vec![];
    for column in columns.as_vec().unwrap() {
        let column_def = column.as_hash().unwrap();
        let mut name = "";
        let mut regex = "";
        for validation in column_def.iter() {
            let key = validation.0.as_str().unwrap();
            let value = validation.1.as_str().unwrap();

            if key == "name" {
                name = value;
                column_names.push(name);
            }
            if key == "regex" {
                regex = value;
            }
        }
        let new_validation = ColumnValidation { column_name: name.to_string(), validation: Regex::new(&regex).unwrap() };
        validations.push(new_validation);
    }

    validations
}

fn validate_column_names(reader: &mut Reader<Box<dyn Read>>, validations: &Vec<ColumnValidation>) -> bool {
    let column_names = validations.iter()
        .map(|v| v.column_name.clone())
        .collect::<Vec<String>>();
    info!("Expected Column Names: {:?}", column_names);

    let headers: Vec<&str> = reader.headers().unwrap().iter().collect();
    info!("Actual Column Names: {:?}", headers);

    column_names == headers
}
/// A Python module implemented in Rust.
#[pymodule]
fn csv_validation(m: &Bound<'_, PyModule>) -> PyResult<()> {
    pyo3_log::init();
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    Ok(())
}


#[cfg(test)]
mod tests {
    use simple_logger::SimpleLogger;
    use crate::validate;

    #[test]
    fn test_validate_csv() {
        SimpleLogger::new().init().unwrap();
        assert!(validate("test/test_file.csv", "test/test_validations.yml").unwrap());
    }

    #[test]
    fn test_validate_csv_gz() {
        assert!(validate("test/test_file.csv.gz", "test/test_validations.yml").unwrap());
    }
}