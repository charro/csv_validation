use crate::Validation::Values;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::io::{BufReader, Read};
use std::iter::zip;
use std::path::Path;
use csv::{Reader, ReaderBuilder};
use flate2::bufread::GzDecoder;
use log::{debug, error, info};
use pyo3::exceptions::{PyRuntimeError};
use pyo3::prelude::*;
use regex::Regex;
use yaml_rust2::{Yaml, YamlLoader};
use serde::{Deserialize, Serialize};
use crate::Validation::{RegularExpression};

const MAX_SAMPLE_SIZE:u16 = 10;
const DEFAULT_COLUMN_SEPARATOR:u8 = b',';
const DEFAULT_DECIMAL_SEPARATOR:char = '.';

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Validation {
    RegularExpression { expression: String, alias: String },
    Min(f64),
    Max(f64),
    Values(Vec<String>),
    None
}

impl PartialEq for Validation {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            // Compare RegularExpression enum values deeply
            (
                Validation::RegularExpression { expression: exp1, alias: alias1 },
                Validation::RegularExpression { expression: exp2, alias: alias2 },
            ) => exp1 == exp2 && alias1 == alias2,

            // Compare Min and Max enums with `f64` values, using custom comparison
            (Validation::Min(v1), Validation::Min(v2)) | (Validation::Max(v1), Validation::Max(v2)) => {
                (v1 - v2).abs() < f64::EPSILON // Tolerates small differences in floats
            },

            // Compare Values variants by comparing the vectors
            (Validation::Values(values1), Validation::Values(values2)) => {
                values1 == values2
            },

            // Compare None variants (no associated data)
            (Validation::None, Validation::None) => true,

            // If enums don't match, they are not equal
            _ => false,
        }
    }
}


#[derive(Clone)]
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

#[derive(Serialize, Deserialize)]
struct FileValidationSummary {
    file_path: String,
    total_rows: usize,
    total_columns: usize,
    correct_columns: Vec<ColumnValidationsSummary>,
    wrong_columns: Vec<ColumnValidationsSummary>
}

impl ValidationSummary {
    fn get_wrong_values_details(&self) -> String {
        let wrong_samples = &self.wrong_values_sample
            .iter()
            .map(|s| format!("'{}'", s))
            .collect::<Vec<String>>()
            .join(",");
        format!("Wrong Rows: {} | Sample: {}", self.wrong_rows, wrong_samples)
    }
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

fn get_regex_string_for_values(values: &Vec<String>) -> String {
    format!("^$|^(?:{})$", values.iter().map(|s| regex_escape(s)).collect::<Vec<_>>().join("|"))
}

fn regex_escape(text: &str) -> String {
    let metacharacters = r"\.^$|()?*+[]{}";
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if metacharacters.contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Infers the file compression status and returns the corresponding buffered reader
fn get_reader_from(path: &str, separator: u8) -> PyResult<Reader<Box<dyn Read>>> {
    let buf_reader = BufReader::new(File::open(Path::new(path))?);
    if is_gzip_file(path)? {
        debug!("File is gzipped");
        let read_capacity = 10 * 1024_usize.pow(2);
        let reader = BufReader::with_capacity(read_capacity, GzDecoder::new(buf_reader));
        Ok(ReaderBuilder::new()
            .delimiter(separator)
            .from_reader(Box::new(reader)))
    }
    else {
        Ok(ReaderBuilder::new()
            .delimiter(separator)
            .from_reader(Box::new(buf_reader)))
    }
}

fn is_gzip_file(path: &str) -> PyResult<bool> {
    let mut bytes = [0u8; 2];
    File::open(Path::new(path))?.read_exact(&mut bytes)?;
    Ok(bytes[0] == 0x1f && bytes[1] == 0x8b)
}

fn get_validations(definition_string: &str) -> PyResult<Vec<ColumnValidations>> {
    // Read the YAML definition with the validations
    let config = YamlLoader::load_from_str(definition_string)
        .map_err(|e| PyRuntimeError::new_err(format!("Invalid YAML format: {}", e)))?;
    
    if config.is_empty() {
        return Err(PyRuntimeError::new_err("Empty YAML definition"));
    }

    let columns = &config[0]["columns"];
    if !columns.is_array() {
        return Err(PyRuntimeError::new_err("Invalid YAML: 'columns' key must be an array"));
    }

    let columns_vec = columns.as_vec()
        .ok_or_else(|| PyRuntimeError::new_err("Invalid YAML: 'columns' must be an array"))?;

    if columns_vec.is_empty() {
        return Err(PyRuntimeError::new_err("No columns defined in YAML"));
    }

    let mut column_names = vec![];
    let mut column_validations = vec![];

    for column in columns_vec {
        let column_def = column.as_hash()
            .ok_or_else(|| PyRuntimeError::new_err("Invalid YAML: each column must be a mapping"))?;
        
        let mut column_name = "";
        let mut validations = vec![];

        for validation_definition in column_def.iter() {
            let key = validation_definition.0.as_str()
                .ok_or_else(|| PyRuntimeError::new_err("Invalid YAML: validation key must be a string"))?;
            let value = validation_definition.1;
            
            let validation = match key {
                "name" => {
                    column_name = value.as_str()
                        .ok_or_else(|| PyRuntimeError::new_err("Column name must be a string"))?;
                    column_names.push(column_name);
                    Ok(Validation::None)
                }
                "regex" => {
                    let expr = value.as_str()
                        .ok_or_else(|| PyRuntimeError::new_err("Regex must be a string"))?;
                    Ok(Validation::RegularExpression { 
                        expression: String::from(expr), 
                        alias: String::from("regex") 
                    })
                }
                "min" => {
                    let num = yaml_value_as_f64(value)
                        .ok_or_else(|| PyRuntimeError::new_err(format!("Min value {:?} is not a number", value)))?;
                    Ok(Validation::Min(num))
                }
                "max" => {
                    let num = yaml_value_as_f64(value)
                        .ok_or_else(|| PyRuntimeError::new_err(format!("Max value {:?} is not a number", value)))?;
                    Ok(Validation::Max(num))
                }
                "values" => {
                    let values = value.as_vec()
                        .ok_or_else(|| PyRuntimeError::new_err("Values must be an array"))?
                        .iter()
                        .map(|v| v.as_str()
                            .ok_or_else(|| PyRuntimeError::new_err("Each value in values array must be a string"))
                            .map(String::from))
                        .collect::<Result<Vec<String>, _>>()?;
                    Ok(Validation::Values(values))
                }
                "format" => {
                    let format = value.as_str()
                        .ok_or_else(|| PyRuntimeError::new_err("Format must be a string"))?;
                    let regex_for_format = get_regex_for_format(format)?;
                    Ok(Validation::RegularExpression { 
                        expression: regex_for_format, 
                        alias: format.to_string() 
                    })
                }
                _ => Err(PyRuntimeError::new_err(format!("Unknown validation: {key}")))
            }?;

            if key != "name" {
                validations.push(validation);
            }
        }

        if column_name.is_empty() {
            return Err(PyRuntimeError::new_err("Each column must have a name"));
        }

        let new_validations = ColumnValidations { column_name: column_name.to_string(), validations };
        column_validations.push(new_validations);
    }

    // If the global flag empty_not_ok is present, we automatically add an extra validation in each column to check that
    // there are not empty values on that column
    let empty_not_ok =
        config[0]["empty_not_ok"].as_bool().map_or(false, |value| value);
    if empty_not_ok {
        let non_empty_validation = RegularExpression {
            expression: String::from("^.+$"),
            alias: String::from("non_empty")
        };
        for column_validations in column_validations.iter_mut() {
            column_validations.validations.push(non_empty_validation.clone());
        }
    }

    Ok(column_validations)
}

fn yaml_value_as_f64(val: &Yaml) -> Option<f64> {
    match val {
        Yaml::Real(s) => s.parse().ok(),
        Yaml::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn get_regex_for_format(format: &str) -> PyResult<String> {
    match format {
        "integer" => Ok(String::from("^$|^-?\\d+$")),
        "positive integer" => Ok(String::from("^$|^\\d+$")),
        "decimal" => Ok(String::from("^$|^-?\\d+(\\.\\d+)?$")),
        "positive decimal point" | "positive decimal" => Ok(String::from("^$|^\\d+(\\.\\d+)?$")),
        "positive decimal comma" => Ok(String::from("^$|^\\d+(,\\d+)?$")),
        "decimal scientific" => Ok(String::from("^$|^-?\\d+(\\.\\d+)?([eE][-+]?\\d+)?$")),
        "non_empty" => Ok(String::from("^.+$")),
        _ => Err(PyRuntimeError::new_err(format!("Unknown format: {format}")))
    }
}

fn validate_column_names(reader: &mut Reader<Box<dyn Read>>, validations: &Vec<ColumnValidations>) -> PyResult<bool> {
    info!("Validating column names and order...");
    let expected_column_names = validations.iter()
        .map(|v| v.column_name.clone())
        .collect::<Vec<String>>();
    debug!("Expected Column Names: {:?}", expected_column_names);

    let headers: Vec<String> = reader.headers().unwrap().iter().map(|s| String::from(s) ).collect();
    debug!("Actual Column Names: {:?}", headers);

    if expected_column_names != headers {
        if expected_column_names.len() != headers.len() {
            info!("The number of columns in the CSV file doesn't match the validations:");
            let expected_columns_set: HashSet<String> = expected_column_names.iter().cloned().collect();
            let headers_set: HashSet<String> = headers.iter().cloned().collect();
            info!("These column names in the CSV file were not expected: {:?}", headers_set.difference(&expected_columns_set));
            info!("These expected column names were missing in the CSV file: {:?}", expected_columns_set.difference(&headers_set));
        }
        else {
            info!("The CSV file has the same number of columns than the validations but some names are different:");
            for (expected_column, header) in zip(expected_column_names, headers) {
                if expected_column != header {
                    info!("{:?} != {:?}", expected_column, header);
                }
            }
        }
        return Ok(false)
    }
    Ok(true)
}

#[pyclass]
struct CSVValidator {
    validations: Vec<ColumnValidations>,
    regex_map: HashMap<String, Regex>,
    separator: u8,
    decimal_separator: char
}

// Methods Exposed to Python
#[pymethods]
impl CSVValidator {
    #[new]
    fn new() -> Self {
        CSVValidator {
            validations: Vec::new(),
            regex_map: HashMap::new(),
            separator: DEFAULT_COLUMN_SEPARATOR,
            decimal_separator: DEFAULT_DECIMAL_SEPARATOR
        }
    }

    #[staticmethod]
    #[pyo3(text_signature = "(definition_path)")]
    /// Create a new CSVValidator from a YAML file with the validation definition.
    ///
    /// Args:
    ///     definition_path (str): The path to the YAML file with the validation definition.
    ///
    /// Returns:
    ///     a CSVValidation instance
    fn from_file(definition_path: &str) -> PyResult<Self> {
        let definition_string = fs::read_to_string(definition_path)?;
        Self::from_string(&definition_string)
    }

    #[staticmethod]
    #[pyo3(text_signature = "(definition_string)")]
    /// Create a new CSVValidator from a YAML string with the validation definition.
    ///
    /// Args:
    ///     definition_string (str): A string with the YAML validations definition.
    ///
    /// Returns:
    ///     a CSVValidation instance
    fn from_string(definition_string: &str) -> PyResult<Self> {
        let validations = get_validations(definition_string)?;
        let mut regex_map = HashMap::new();
        
        // Pre-Compile and save all Regex expressions
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

        Ok(CSVValidator { validations, regex_map, separator: DEFAULT_COLUMN_SEPARATOR, decimal_separator: DEFAULT_DECIMAL_SEPARATOR })
    }

    #[pyo3(text_signature = "(self, separator)")]
    /// Set the CSV separator (also known as delimiter):
    ///
    /// The separator is ',' by default. If your file has a different one, you can modify it here.
    ///
    /// Args:
    ///     separator (str): A string with only one character that is the CSV file separator.
    fn set_separator(&mut self, separator: String)  -> PyResult<()> {
        if separator.len() == 1 {
            self.separator = separator.chars().next().unwrap() as u8;
            Ok(())
        }
        else {
            Err(PyRuntimeError::new_err(format!("Wrong separator {separator}. It can only have one character")))
        }
    }

    #[pyo3(text_signature = "(self, decimal_separator)")]
    /// Set the decimal number separator
    ///
    /// The default decimals separator for float numbers is '.'.
    /// If your file uses a different one, you can modify it here.
    ///
    /// Args:
    ///     decimal_separator (str): A string with only one character that is the decimal separator.
    fn set_decimal_separator(&mut self, decimal_separator: String)  -> PyResult<()> {
        if decimal_separator.len() == 1 {
            self.decimal_separator = decimal_separator.chars().next().unwrap();
            Ok(())
        }
        else {
            Err(PyRuntimeError::new_err(format!("Wrong decimal separator {decimal_separator}. It can only have one character")))
        }
    }

    #[pyo3(text_signature = "(self, file_path)")]
    /// Validates the CSV file against the validations defined in this CSVValidator.
    /// When finished, it will log a comprehensive summary with the details of the Validation.
    ///
    /// Args:
    ///     file_path (str): The path with the CSV file to validate.
    ///
    /// Returns:
    ///     bool: True if the file fully complies with all the validations, False otherwise
    fn validate(&self, file_path: &str) -> PyResult<bool> {
        // Build the CSV reader
        let mut rdr = get_reader_from(file_path, self.separator)?;

        // First validation: Ensure column names and order are exactly as expected
        if validate_column_names(&mut rdr, &self.validations)? {
            info!("Columns names and order are correct");
        }
        else {
            error!("Expected columns != File columns. Cannot continue with rest of the validations");
            return Ok(false);
        }

        // Second validation: If column names match, check if also the values match the validations
        let mut validation_summaries_map = build_validation_summaries_map(&self.validations);
        let mut is_valid_file = true;
        let mut validated_rows = 0;

        // Iterate over the CSV file and validate each row
        for result in rdr.records() {
            let record = result.unwrap();
            for next_column in zip(record.iter(), self.validations.iter()) {
                let value = next_column.0;
                let _column_name = &next_column.1.column_name;
                for validation in &next_column.1.validations {
                    let valid = self.apply_validation(value, validation, &self.regex_map)?;
                    if !valid {
                        let validation_summary_list = validation_summaries_map.get_mut(_column_name).unwrap();
                        let validation_summary = validation_summary_list
                            .iter_mut()
                            .find(|val_sum| val_sum.validation == *validation)
                            .unwrap();

                        validation_summary.wrong_rows += 1;
                        if validation_summary.wrong_values_sample.len() < MAX_SAMPLE_SIZE as usize {
                            validation_summary.wrong_values_sample.push(value.to_string());
                        }
                    }
                    is_valid_file = is_valid_file && valid;
                }
            }
            validated_rows = validated_rows + 1;
        }

        // Fill the ColumnValidationSummary for each column
        let mut column_validation_summaries = Vec::new();
        for column_validation in &self.validations {
            let validation_summary_for_column =
                validation_summaries_map.remove(&column_validation.column_name).unwrap();
            let column_validation_summary = ColumnValidationsSummary {
                column_name: column_validation.column_name.clone(),
                validation_summaries: validation_summary_for_column
            };
            column_validation_summaries.push(column_validation_summary);
        }

        let validation_process_summary =
            &self.create_validation_process_summary(column_validation_summaries, validated_rows, file_path);

        // TODO: Decide how to return the results of the validations in JSON
        //let _validation_result_json = serde_json::to_string(&validation_process_summary).unwrap();

        info!("");
        info!("VALIDATIONS SUMMARY");
        info!("==================================================");
        info!("FILE: {}", file_path);
        info!("Rows: {} | Columns: {}", validation_process_summary.total_rows, validation_process_summary.total_columns);

        let correct_columns = validation_process_summary.correct_columns.len();
        let wrong_columns = validation_process_summary.wrong_columns.len();

        info!("");
        info!("CORRECT COLUMNS: {}/{}", correct_columns, validation_process_summary.total_columns);
        info!("--------------------------------------------------");
        for column_validation_summary in &validation_process_summary.correct_columns {
            info!("  - {}: [✔] OK", column_validation_summary.column_name);
            for validation_summary in column_validation_summary.validation_summaries.iter() {
                info!("      ✔ - {:?}", validation_summary.validation);
            }
        }

        info!("");
        info!("WRONG COLUMNS: {}/{}", wrong_columns, validation_process_summary.total_columns);
        info!("--------------------------------------------------");
        for column_validation_summary in &validation_process_summary.wrong_columns {
            info!("  - {}: [✖] FAIL", column_validation_summary.column_name);
            for validation_summary in column_validation_summary.validation_summaries.iter() {
                if validation_summary.wrong_rows > 0 {
                    info!("      ✖ - {:?}", validation_summary.validation);
                    info!("          {:?}", validation_summary.get_wrong_values_details());
                }
                else {
                    info!("      ✔ - {:?}", validation_summary.validation);
                }
            }
        }

        info!("");
        info!("VALIDATION RESULT");
        info!("--------------------------------------------------");
        if is_valid_file {
            info!("[✔] OK - File matches the validations");
        }
        else {
            info!("[✖] FAIL: File DOESN'T match all validations");
        }
        info!("");
        Ok(is_valid_file)
    }
}

// Internal methods that are NOT Exposed in Python
impl CSVValidator {
    fn apply_validation(&self, value: &str, validation: &Validation, regex_map: &HashMap<String, Regex>) -> PyResult<bool> {
        match validation {
            RegularExpression { expression: exp, alias: _ } => {
                let regex = regex_map.get(exp).unwrap();
                Ok(regex.is_match(value))
            },
            Validation::Min(min) => {
                if value.is_empty() {
                    Ok(true)
                }
                else {
                    let normalized_value = value.replace(self.decimal_separator, ".");
                    match normalized_value.parse::<f64>() {
                        Ok(value) => Ok(value >= *min),
                        Err(_) => Ok(false)
                    }
                }
            },
            Validation::Max(max) => {
                if value.is_empty() {
                    Ok(true)
                }
                else {
                    let normalized_value = value.replace(self.decimal_separator, ".");
                    match normalized_value.parse::<f64>() {
                        Ok(value) => Ok(value <= *max),
                        Err(_) => Ok(false)
                    }
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

    fn create_validation_process_summary(&self, column_validation_summaries: Vec<ColumnValidationsSummary>, validated_rows: usize, file_path: &str) -> FileValidationSummary {
        let total_rows = validated_rows;
        let total_columns = column_validation_summaries.len();

        let mut failed_columns = HashSet::new();
        let mut correct_columns = vec![];
        let mut wrong_columns = vec![];
        for column_validation_summary in column_validation_summaries {
            for validation_summary in &column_validation_summary.validation_summaries {
                if validation_summary.wrong_rows > 0 {
                    let column_name = &column_validation_summary.column_name;
                    if !failed_columns.contains(column_name) {
                        failed_columns.insert(column_name.clone());
                    }
                }
            }

            if failed_columns.contains(&column_validation_summary.column_name)  {
                wrong_columns.push(column_validation_summary);
            }
            else {
                correct_columns.push(column_validation_summary);
            }
        }

        FileValidationSummary {
            file_path: file_path.parse().unwrap(),
            total_rows,
            total_columns,
            correct_columns,
            wrong_columns
        }
    }
}

/// A Python module implemented in Rust.
#[pymodule]
fn csv_validation(m: &Bound<'_, PyModule>) -> PyResult<()> {
    pyo3_log::init();
    m.add_class::<CSVValidator>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use simple_logger::SimpleLogger;
    use crate::CSVValidator;

    #[test]
    fn init_logger() {
        SimpleLogger::new().init().unwrap();
    }

    #[test]
    fn test_wrong_headers() {
        let definition = String::from("
            columns:
              - name: First Column
              - name: Second Column
              - name: Different Column name in file
              - name: Missing Column Not In File
        ");
        let validator = CSVValidator::from_string(&definition).unwrap();
        assert!(!validator.validate("test/test_file.csv").unwrap());
    }

    #[test]
    fn test_csv_validator_from_file() {
        let validator = CSVValidator::from_file("test/test_validations.yml").unwrap();
        assert!(validator.validate("test/test_file.csv").unwrap());
    }

    #[test]
    fn test_csv_validator_from_string() {
        let definition = String::from("
            columns:
              - name: First Column
                regex: ^.+$
              - name: Second Column
                regex: ^.+$
              - name: Third Column
                regex: ^-?[0-9]+$
                min: -23
                max: 2000
        ");
        let validator = CSVValidator::from_string(&definition).unwrap();
        assert!(validator.validate("test/test_file.csv").unwrap());
    }

    #[test]
    fn test_csv_validator_reuse() {
        // Test that we can reuse the same validator for multiple files
        let validator = CSVValidator::from_file("test/test_validations.yml").unwrap();
        assert!(validator.validate("test/test_file.csv").unwrap());
        assert!(validator.validate("test/test_file.csv.gz").unwrap());
    }

    #[test]
    fn test_csv_validator_all_validation_types() {
        let definition = String::from("
            columns:
              - name: Name
                regex: ^[A-Za-z\\s]{2,50}$
              - name: Age
                format: positive integer
                min: 0
                max: 120
              - name: Status
                values: [active, inactive, pending]
        ");
        let validator = CSVValidator::from_string(&definition).unwrap();
        // This should fail as test_file.csv doesn't match these validations
        assert!(!validator.validate("test/test_file.csv").unwrap());
    }

    #[test]
    fn test_empty_ok_by_default() {
        let definition_empty_ok = String::from("
            columns:
              - name: First Column
                format: positive integer
              - name: Second Column
                values: ['A', 'B', 'C']
        ");
        let validator = CSVValidator::from_string(&definition_empty_ok).unwrap();
        // This is OK as we didn't mention anything about empty values
        assert!(validator.validate("test/empty_values.csv").unwrap());
    }

    #[test]
    fn test_empty_not_ok() {
        let definition_empty_ok = String::from("
            empty_not_ok: true
            columns:
              - name: First Column
                format: positive integer
              - name: Second Column
                values: ['A', 'B', 'C']
        ");
        let validator = CSVValidator::from_string(&definition_empty_ok).unwrap();
        // Validation is not OK as the file contains empty values
        assert!(!validator.validate("test/empty_values.csv").unwrap());
    }

    #[test]
    fn test_csv_validator_invalid_format() {
        let definition = String::from("
            columns:
              - name: First Column
                format: unknown_format  # This format doesn't exist
        ");
        assert!(CSVValidator::from_string(&definition).is_err());
    }

    #[test]
    fn test_csv_validator_invalid_file() {
        let validator = CSVValidator::from_file("test/test_validations.yml").unwrap();
        assert!(validator.validate("nonexistent_file.csv").is_err());
    }

    #[test]
    fn test_csv_validator_invalid_yaml() {
        let definition = String::from("
            invalid:
              yaml:
                format
        ");
        assert!(CSVValidator::from_string(&definition).is_err());
    }

    #[test]
    fn test_csv_validator_empty_definition() {
        let definition = String::from("");
        assert!(CSVValidator::from_string(&definition).is_err());
    }

    #[test]
    fn test_csv_validator_min_max_validation() {
        let definition = String::from("
            columns:
              - name: First Column
              - name: Second Column
              - name: Third Column
                min: -23
                max: 1978
        ");
        let validator = CSVValidator::from_string(&definition).unwrap();
        assert!(validator.validate("test/test_file.csv").unwrap());
    }

    #[test]
    fn test_csv_validator_min_max_validation_empty_values_are_ok_by_default() {
        let definition = String::from("
            columns:
              - name: First Column
                min: -23
                max: 1978
              - name: Second Column
        ");
        let validator = CSVValidator::from_string(&definition).unwrap();
        assert!(validator.validate("test/empty_values.csv").unwrap());
    }

    #[test]
    fn test_list_of_values_with_special_characters() {
        let definition = String::from("
            columns:
              - name: ColumnA
                values: ['A+', '[B]', 'C$']
        ");

        let path_of_file_to_validate = "test/test_file_with_special_characters.csv";
        let file_content = "ColumnA\nA+\n[B]\nC$";
        std::fs::write(path_of_file_to_validate, file_content).unwrap();

        let validator = CSVValidator::from_string(&definition).unwrap();
        assert!(validator.validate(path_of_file_to_validate).unwrap());
        std::fs::remove_file(path_of_file_to_validate).unwrap();
    }

    #[test]
    fn test_min_max_with_custom_decimal_separator() {
        let definition = String::from("
            columns:
              - name: ColumnA
                min: -5
                max: 27.8
        ");

        let path_of_file_to_validate = "test/test_file_with_custom_decimals.csv";
        let file_content = "ColumnA\n0\n4,3\n27,8";
        std::fs::write(path_of_file_to_validate, file_content).unwrap();

        let mut validator = CSVValidator::from_string(&definition).unwrap();
        validator.set_separator(String::from(";")).unwrap();
        validator.set_decimal_separator(String::from(",")).unwrap();
        assert!(validator.validate(path_of_file_to_validate).unwrap());
        std::fs::remove_file(path_of_file_to_validate).unwrap();
    }

    #[test]
    fn test_format_integer() {
        let test_cases = vec![
            ("42", true),
            ("-42", true),
            ("0", true),
            ("", true),      // Empty values are allowed by default
            ("3.14", false),
            ("abc", false),
            ("123abc", false),
        ];

        test_format_validation("integer", test_cases);
    }

    #[test]
    fn test_format_positive_integer() {
        let test_cases = vec![
            ("42", true),
            ("0", true),
            ("", true),      // Empty values are allowed by default
            ("-42", false),
            ("3.14", false),
            ("abc", false),
            ("123abc", false),
        ];

        test_format_validation("positive integer", test_cases);
    }

    #[test]
    fn test_format_decimal() {
        let test_cases = vec![
            ("42", true),
            ("42.0", true),
            ("42.42", true),
            ("-42.42", true),
            ("0.0", true),
            ("", true),      // Empty values are allowed by default
            ("abc", false),
            ("12.34.56", false),
            ("12e4", false),
        ];

        test_format_validation("decimal", test_cases);
    }

    #[test]
    fn test_format_positive_decimal() {
        let test_cases = vec![
            ("42", true),
            ("42.0", true),
            ("42.42", true),
            ("0.0", true),
            ("", true),      // Empty values are allowed by default
            ("-42.42", false),
            ("abc", false),
            ("12.34.56", false),
            ("12e4", false),
        ];

        test_format_validation("positive decimal", test_cases);
    }

    #[test]
    fn test_format_decimal_scientific() {
        let test_cases = vec![
            ("42", true),
            ("42.0", true),
            ("42.42", true),
            ("-42.42", true),
            ("1.234e5", true),
            ("1.234e+5", true),
            ("1.234e-5", true),
            ("1.234E5", true),
            ("", true),      // Empty values are allowed by default
            ("abc", false),
            ("12.34.56", false),
            ("1.234e", false),
            ("e5", false),
            ("1.234e+", false),
        ];

        test_format_validation("decimal scientific", test_cases);
    }
    
    // Helper function to test format validations
    fn test_format_validation(format: &str, test_cases: Vec<(&str, bool)>) {
        let definition = format!("
            columns:
              - name: Test
                format: {}
        ", format);

        let validator = CSVValidator::from_string(&definition).unwrap();

        for (value, expected) in test_cases {
            let test_csv = format!("Test\n{}", value);
            let temp_file = format!("test/temp_format_test_{}.csv", format.replace(" ", "_"));
            std::fs::write(&temp_file, test_csv).unwrap();
            
            let result = validator.validate(&temp_file).unwrap();
            assert_eq!(
                result, 
                expected, 
                "Format '{}' validation failed for value '{}': expected {}, got {}", 
                format, value, expected, result
            );

            std::fs::remove_file(&temp_file).unwrap();
        }
    }
}