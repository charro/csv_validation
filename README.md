# CSV Validation

Blazing fast format validations for your CSV files

This is a Python lib with a Rust core that will allow you to validate huge CSV files (GBs) in seconds (or in few minutes for really huge files) using a minimal amount of memory.

## Features

- ✨ Validate both plain and gzipped CSV files
- 🔍 Multiple validation types supported:
  - Regular expressions
  - Minimum/Maximum value checks
  - Value set validation (allowed values)
  - Column name and order validation
- 🐍 Python bindings included
- 📝 Detailed validation summaries with sample invalid values
- 🚀 High performance with optimizations like regex pre-compilation
- 📊 Support for large CSV files

## Installation

### Python

```bash
pip install csv_validation
```

## Usage

### Python

#### You can provide a file with the validation rules
```python
from csv_validation import CSVValidator

validator = CSVValidator.from_file("validation_rules.yaml")
is_valid = validator.validate("data.csv")
```

#### You can also create a validator from a string
```python
validation_rules = """
columns:
  - name: Name
    regex: ^[A-Za-z\s]{2,50}$
  - name: Age
    format: positive_integer
    max: 120
"""

validator = CSVValidator.from_string(validation_rules)
is_valid = validator.validate("data.csv")
```

### Validation Definition Format

Create a small, easy to read YAML file with your validation rules. Example:

```yaml
columns:
  - name: Name
    regex: ^[A-Za-z\s]{2,50}$  # Letters and spaces, 2-50 characters
  - name: Family Name
    regex: ^[A-Za-z\s'-]{2,50}$  # Letters, spaces, hyphens and apostrophes
  - name: Age
    format: positive_integer  # Using predefined format instead of custom regex
    max: 120
  - name: Salary
    format: integer  # Allows negative integers too
    min: 20000
  - name: Email
    regex: ^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$  # Standard email format
  - name: Phone
    regex: ^\+?[0-9]{10,15}$  # International phone format with optional +
  - name: Status
    values: [active, inactive, pending, suspended]  # Only these values are allowed
  - name: Gender
    values: [M, F, NB, O]  # M: Male, F: Female, NB: Non-Binary, O: Other
```

## Validation Types

1. **Regular Expression** (`regex`)
   - Validate fields against custom regex patterns

2. **Format** (`format`)
   - Predefined formats for common validations
   - Available formats:
     - `integer`: Validates any integer number (positive or negative)
     - `positive_integer`: Validates positive integer numbers
     - `non_empty`: Validates that field contains at least one character
   - More formats will be added in next versions

3. **Minimum Value** (`min`)
   - Check if numeric fields are greater than or equal to a specified value

4. **Maximum Value** (`max`)
   - Check if numeric fields are less than or equal to a specified value

5. **Value Set** (`values`)
   - Ensure fields only contain values from a predefined set

## Global Validations

### empty_not_ok

Empty values (empty string '') are considered correct and accepted by default. 
However, if your data must always have some content, 
you can add a global `empty_not_ok` flag at the root level of your YAML definition to automatically 
add the `format: non_empty` validation to all columns:

```yaml
empty_not_ok: true
columns:
  - name: Column1
    # other validations...
```

## Output

The validation process provides detailed feedback including:
- Overall validation status (true/false)
- Number of invalid rows per validation rule
- Sample of invalid values for debugging
- Detailed logging for troubleshooting

## Development

### Prerequisites

- Rust 2021 edition or later
- Python 3.6+ (for Python bindings)
- Cargo and standard Rust tooling

### Building from Source

```bash
# Clone the repository
git clone https://github.com/charro/csv_validation
cd csv_validation

# Build the project
cargo build --release

# Run tests
cargo test
```

## Dependencies

- `csv`: CSV parsing
- `flate2`: Compression support
- `pyo3`: Python bindings
- `regex`: Regular expression support
- `yaml-rust2`: YAML parsing
- Various utilities for logging and serialization

## License

MIT License

Copyright (c) 2024 CSV Validation Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.