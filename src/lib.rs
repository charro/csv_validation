use crate::Validation::Values;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::io::{BufReader, Read};
use std::iter::zip;
use std::path::Path;
use csv::Reader;
use flate2::bufread::GzDecoder;
use log::{debug, error, info};
use pyo3::exceptions::{PyRuntimeError};
use pyo3::prelude::*;
use regex::Regex;
use yaml_rust2::YamlLoader;
use serde::{Deserialize, Serialize};
use crate::Validation::{RegularExpression};

const MAX_SAMPLE_SIZE:u16 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Validation {
    RegularExpression { expression: String, alias: String },
    Min(f64),
    Max(f64),
    Values(Vec<String>),
    None
}

struct ColumnValidations {
    column_name: String,
    validations: Vec<Validation>
}

#[derive(Serialize, Deserialize)]
struct ValidationSummary {
    validation: Validation,
    wrong_rows: usize,
    wrong_values_sample: Vec<String>
}

#[derive(Serialize, Deserialize)]
struct ColumnValidationsSummary {
    column_name: String,
    validation_summaries: Vec<ValidationSummary>
}

#[pyfunction]
fn validate_with_file(path: &str, definition_path: &str) -> PyResult<bool> {
    info!("Validating file {} against definition file {}", path, definition_path);
    let definition_string = fs::read_to_string(definition_path)?;
    validate(path, definition_string)
}

/// Validate that CSV file complies with a validations definition file
#[pyfunction]
fn validate(path: &str, definition_string: String) -> PyResult<bool> {
    debug!("Validating file {} against definition:\n {}", path, definition_string);
    let validations = get_validations(definition_string.as_str())?;

    // Pre-Compile and save all Regex expressions to save time in execution
    let mut regex_map = HashMap::new();
    for column_validation in &validations {
        for validation in &column_validation.validations {
            match validation {
                RegularExpression { expression, alias: _ } => {
                    regex_map.insert(expression.to_string(), Regex::new(expression.as_str()).unwrap());
                },
                Values(values) => {
                    let regex_str = get_regex_string_for_values(values);
                    regex_map.insert(regex_str.clone(), Regex::new(regex_str.as_str()).unwrap());
                },
                _ => continue
            }
        }
    }

    // Build the CSV reader
    let mut rdr = get_reader_from(path)?;

    // First validation: Ensure column names and order are exactly as expected
    if validate_column_names(&mut rdr, &validations)? {
        info!("Columns names and order are correct");
    }
    else {
        error!("Expected columns != Real columns");
        return Ok(false);
    }

    // Second validation: If column names match, check if also the values match the validations
    let mut validation_summaries_map = build_validation_summaries_map(&validations);
    let mut is_valid_file = true;
    for result in rdr.records() {
        let record = result.unwrap();
        for next_column in zip(record.iter(), validations.iter()) {
            let value = next_column.0;
            let _column_name = &next_column.1.column_name;
            for validation in &next_column.1.validations {
                let valid = apply_validation(value, validation, &regex_map)?;
                if !valid {
                    let validation_summary_list = validation_summaries_map.get_mut(_column_name).unwrap();
                    let validation_summary = validation_summary_list
                            .iter_mut()
                            .find(|val_sum|
                                std::mem::discriminant(&val_sum.validation) == std::mem::discriminant(validation)).unwrap();
                    validation_summary.wrong_rows += 1;
                    if validation_summary.wrong_values_sample.len() < MAX_SAMPLE_SIZE as usize {
                        validation_summary.wrong_values_sample.push(value.to_string());
                    }
                }
                is_valid_file = is_valid_file && valid;
            }
        }
    }

    // Fill the ColumnValidationSummary for each column
    let mut column_validation_summaries = Vec::new();
    for column_validation in &validations {
        let validation_summary_for_column =
            validation_summaries_map.remove(&column_validation.column_name).unwrap();
        let column_validation_summary = ColumnValidationsSummary {
            column_name: column_validation.column_name.clone(),
            validation_summaries: validation_summary_for_column
        };
        column_validation_summaries.push(column_validation_summary);
    }

    let validation_result_json = serde_json::to_string(&column_validation_summaries).unwrap();

    debug!("VALIDATIONS SUMMARY");
    debug!("==================================================================================");
    for column_validation_summary in column_validation_summaries {
        debug!("Column: '{}'", column_validation_summary.column_name);
        for validation_summary in column_validation_summary.validation_summaries {
            let wrong_values_sample = if validation_summary.wrong_values_sample.len() > 0 {
                    format!(" | Wrong Values Sample: {:?}", validation_summary.wrong_values_sample)
            } else {
                String::from("" )
            };
            debug!("\tValidation {:?} => Wrong Rows: {}{}", validation_summary.validation,
                validation_summary.wrong_rows, wrong_values_sample);
        }
    }

    if is_valid_file {
        info!("OK: File matches the validations");
    }
    else {
        info!("NO OK: File DOESN'T match validations");
    }
    Ok(is_valid_file)
}

fn build_validation_summaries_map(validations: &Vec<ColumnValidations>) -> HashMap<String, Vec<ValidationSummary>> {
    let mut validation_summaries_map = HashMap::new();
    for validation in validations {
        let mut validation_summaries = Vec::new();
        for column_validation in &validation.validations {
            let validation_summary =
                ValidationSummary{validation: (*column_validation).clone(), wrong_rows: 0, wrong_values_sample: Vec::new()};
            validation_summaries.push(validation_summary);
        }
        validation_summaries_map.insert(validation.column_name.clone(), validation_summaries);
    }

    validation_summaries_map
}

fn apply_validation(value: &str, validation: &Validation, regex_map: &HashMap<String, Regex>) -> PyResult<bool> {
    match validation {
        RegularExpression { expression: exp, alias: _ } => {
            let regex = regex_map.get(exp).unwrap();
            Ok(regex.is_match(value))
        },
        Validation::Min(min) => {
            match value.parse::<f64>() {
                Ok(value) => Ok(value >= *min),
                Err(_) => Ok(false)
            }
        },
        Validation::Max(max) => {
            match value.parse::<f64>() {
                Ok(value) => Ok(value <= *max),
                Err(_) => Ok(false)
            }
        },
        Validation::Values(values) => {
            let regex_str = get_regex_string_for_values(values);
            let regex = regex_map.get(&regex_str).unwrap();
            Ok(regex.is_match(value))
        }
        Validation::None => Err(PyRuntimeError::new_err("'None' validation has no implementation"))
    }
}

fn get_regex_string_for_values(values: &Vec<String>) -> String {
    values.join("|")
}

/// Infers the file compression type and returns the corresponding buffered reader
fn get_reader_from(path: &str) -> PyResult<Reader<Box<dyn Read>>> {
    let buf_reader = BufReader::new(File::open(Path::new(path))?);
    if is_gzip_file(path)? {
        debug!("File is gzipped");
        let read_capacity = 10 * 1024_usize.pow(2);
        let reader = BufReader::with_capacity(read_capacity, GzDecoder::new(buf_reader));
        Ok(Reader::from_reader(Box::new(reader)))
    }
    else {
        Ok(Reader::from_reader(Box::new(buf_reader)))
    }
}

fn is_gzip_file(path: &str) -> PyResult<bool> {
    let mut bytes = [0u8; 2];
    File::open(Path::new(path))?.read_exact(&mut bytes)?;
    Ok(bytes[0] == 0x1f && bytes[1] == 0x8b)
}

fn get_validations(definition_string: &str) -> PyResult<Vec<ColumnValidations>> {
    // Read the YAML definition with the validations
    let config = YamlLoader::load_from_str(definition_string).unwrap();
    // Get the column names list and each associated validation
    let columns = &config[0]["columns"];
    let mut column_names = vec![];
    let mut column_validations = vec![];
    for column in columns.as_vec().unwrap() {
        let column_def = column.as_hash().unwrap();
        let mut column_name = "";
        let mut validations = vec![];
        for validation_definition in column_def.iter() {
            let key = validation_definition.0.as_str().unwrap();
            let value = validation_definition.1;
            let validation = match key {
                    "name" => {
                        column_name = value.as_str().unwrap();
                        column_names.push(column_name);
                        Ok(Validation::None)
                    }
                    "regex" => { Ok(Validation::RegularExpression { expression: String::from(value.as_str().unwrap()), alias: String::from("regex") }) }
                    "min" => { Ok(Validation::Min(value.as_i64().unwrap() as f64)) }
                    "max" => { Ok(Validation::Max(value.as_i64().unwrap() as f64)) }
                    "values" => {
                        Ok(Validation::Values(value.as_vec().unwrap()
                            .iter()
                            .map(|v| String::from(v.as_str().unwrap()))
                            .collect()
                        ))
                    }
                    "format" => {
                        let format = value.as_str().unwrap();
                        let regex_for_format = get_regex_for_format(format)?;
                        Ok(Validation::RegularExpression { expression: regex_for_format, alias: format.to_string() })
                    }
                    _ => Err(PyRuntimeError::new_err(format!("Unknown validation: {key}")))
                }?;

            if key != "name" {
                validations.push(validation);
            }

        }
        let new_validations = ColumnValidations { column_name: column_name.to_string(), validations: validations };
        column_validations.push(new_validations);
    }

    Ok(column_validations)
}

fn get_regex_for_format(format: &str) -> PyResult<String> {
    match format {
        "integer" => Ok(String::from("^-?\\d+$")),
        "positive integer" => Ok(String::from("^\\d+$")),
        _ => Err(PyRuntimeError::new_err(format!("Unknown format: {format}")))
    }
}

fn validate_column_names(reader: &mut Reader<Box<dyn Read>>, validations: &Vec<ColumnValidations>) -> PyResult<bool> {
    let expected_column_names = validations.iter()
        .map(|v| v.column_name.clone())
        .collect::<Vec<String>>();
    debug!("Expected Column Names: {:?}", expected_column_names);

    let headers: Vec<String> = reader.headers().unwrap().iter().map(|s| String::from(s) ).collect();
    debug!("Actual Column Names: {:?}", headers);

    if expected_column_names != headers {
        if expected_column_names.len() != headers.len() {
            let expected_columns_set: HashSet<String> = expected_column_names.iter().cloned().collect();
            let headers_set: HashSet<String> = headers.iter().cloned().collect();
            debug!("File headers not in expected columns: {:?}", headers_set.difference(&expected_columns_set));
            debug!("Columns in expected columns not in file headers: {:?}", expected_columns_set.difference(&headers_set));
        }
        else {
            for (expected_column, header) in zip(expected_column_names, headers) {
                if expected_column != header {
                    debug!("{:?} != {:?}", expected_column, header);
                }
            }
        }
        return Ok(false)
    }
    Ok(true)
}

/// A Python module implemented in Rust.
#[pymodule]
fn csv_validation(m: &Bound<'_, PyModule>) -> PyResult<()> {
    pyo3_log::init();
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    m.add_function(wrap_pyfunction!(validate_with_file, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use simple_logger::SimpleLogger;
    use crate::{validate, validate_with_file};

    #[test]
    fn init_logger() {
        SimpleLogger::new().init().unwrap();
    }

    #[test]
    fn test_validate_csv_with_file() {
        assert!(validate_with_file("test/test_file.csv", "test/test_validations.yml").unwrap());
    }

    #[test]
    fn test_validate_csv_gz_with_file() {
        assert!(validate_with_file("test/test_file.csv.gz", "test/test_validations.yml").unwrap());
    }

    #[test]
    fn test_wrong_headers() {
        let definition = String::from("
            columns:
              - name: First Column
              - name: Second Column
              - name: Wrong Column
              - name: Expected Column Not In File
        ");
        assert!(!validate("test/test_file.csv", definition).unwrap());
    }

    #[test]
    fn test_format_validation() {
        let definition = String::from("
            columns:
              - name: First Column
              - name: Second Column
              - name: Third Column
                format: integer
        ");
        assert!(validate("test/test_file.csv", definition).unwrap());
    }

    #[test]
    fn test_validate_csv() {
        let definition = String::from("
            columns:
              - name: First Column
                regex: ^.+$
              - name: Second Column
                values: [one_value, or_another]
              - name: Third Column
                regex: ^-?[0-9]+$
                min: -23
                max: 2000
        ");

        assert!(validate("test/test_file.csv", definition).unwrap());
    }
}