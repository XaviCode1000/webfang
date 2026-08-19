//! Zero-dep JSON-schema subset validator (#789, design D3).
//!
//! Covers `type` / `required` / `properties` / `enum` / `items`; paths are
//! reported as `$.a.b[0].c`. NOT a replacement for the `jsonschema` crate
//! (no `allOf`/`$ref`/`format`).

use serde_json::Value;

/// Schema-subset validation errors (paths as `$.a.b[0].c`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// Instance type differs from the schema's `type`.
    TypeMismatch {
        /// JSON path of the offending value.
        path: String,
        /// Expected JSON-schema type.
        expected: String,
        /// Actual JSON type found.
        actual: String,
    },
    /// A `required` property is absent.
    MissingRequired {
        /// JSON path of the object missing the field.
        path: String,
        /// Name of the missing required property.
        field: String,
    },
    /// A value is not one of the schema's `enum` values.
    NotInEnum {
        /// JSON path of the offending value.
        path: String,
    },
    /// An array item violates the `items` type.
    ItemsTypeMismatch {
        /// JSON path of the array.
        path: String,
        /// Index of the offending item.
        idx: usize,
    },
    /// The user-provided schema itself is invalid.
    InvalidSchema {
        /// Reason (Spanish, user-facing).
        msg: String,
    },
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TypeMismatch {
                path,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "tipo inválido en {path}: se esperaba {expected}, se recibió {actual}"
                )
            },
            Self::MissingRequired { path, field } => {
                write!(f, "falta el campo requerido '{field}' en {path}")
            },
            Self::NotInEnum { path } => write!(f, "valor fuera de enum en {path}"),
            Self::ItemsTypeMismatch { path, idx } => {
                write!(f, "elemento [{idx}] de {path} tiene tipo inválido")
            },
            Self::InvalidSchema { msg } => write!(f, "esquema inválido: {msg}"),
        }
    }
}

/// Validate the user-provided schema itself (recursive; #789 R4).
///
/// # Errors
///
/// Returns [`SchemaError::InvalidSchema`] when `type` is missing or
/// unsupported at any level.
pub fn validate_schema(schema: &Value) -> Result<(), SchemaError> {
    walk_schema(schema, "$")
}

/// Validate one record instance against a (pre-validated) schema.
///
/// # Errors
///
/// Returns the first [`SchemaError`] found; never a silent null.
pub fn validate_record(record: &Value, schema: &Value) -> Result<(), SchemaError> {
    walk_record(record, schema, "$".to_string())
}

fn walk_schema(node: &Value, path: &str) -> Result<(), SchemaError> {
    let ty =
        node.get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| SchemaError::InvalidSchema {
                msg: format!("el campo 'type' es obligatorio en {path}"),
            })?;
    match ty {
        "object" => {
            if let Some(props) = node.get("properties").and_then(Value::as_object) {
                for (key, sub) in props {
                    walk_schema(sub, &format!("{path}.{key}"))?;
                }
            }
        },
        "array" => {
            if let Some(items) = node.get("items") {
                walk_schema(items, &format!("{path}.items"))?;
            }
        },
        "string" | "number" | "integer" | "boolean" | "null" => {},
        other => {
            return Err(SchemaError::InvalidSchema {
                msg: format!("tipo no soportado '{other}' en {path}"),
            });
        },
    }
    if node.get("enum").is_some_and(|e| !e.is_array()) {
        return Err(SchemaError::InvalidSchema {
            msg: format!("'enum' debe ser un array en {path}"),
        });
    }
    Ok(())
}

fn actual_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(v: &Value, expected: &str) -> bool {
    match expected {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => false,
    }
}

fn walk_record(value: &Value, schema: &Value, path: String) -> Result<(), SchemaError> {
    let ty = schema.get("type").and_then(Value::as_str).unwrap_or("null");
    match ty {
        "object" => {
            let obj = value.as_object().ok_or_else(|| SchemaError::TypeMismatch {
                path: path.clone(),
                expected: "object".to_string(),
                actual: actual_type(value).to_string(),
            })?;
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !obj.contains_key(name) {
                        return Err(SchemaError::MissingRequired {
                            path: path.clone(),
                            field: name.to_string(),
                        });
                    }
                }
            }
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (key, sub) in props {
                    if let Some(v) = obj.get(key) {
                        walk_record(v, sub, format!("{path}.{key}"))?;
                    }
                }
            }
        },
        "array" => {
            let arr = value.as_array().ok_or_else(|| SchemaError::TypeMismatch {
                path: path.clone(),
                expected: "array".to_string(),
                actual: actual_type(value).to_string(),
            })?;
            if let Some(expected) = schema
                .get("items")
                .and_then(|i| i.get("type"))
                .and_then(Value::as_str)
            {
                for (idx, item) in arr.iter().enumerate() {
                    if !type_matches(item, expected) {
                        return Err(SchemaError::ItemsTypeMismatch {
                            path: path.clone(),
                            idx,
                        });
                    }
                }
            }
        },
        simple if type_matches(value, simple) => {},
        _ => {
            return Err(SchemaError::TypeMismatch {
                path,
                expected: ty.to_string(),
                actual: actual_type(value).to_string(),
            });
        },
    }
    if let Some(vals) = schema.get("enum").and_then(Value::as_array) {
        if !vals.contains(value) {
            return Err(SchemaError::NotInEnum { path });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_str, json};

    const SCHEMA: &str = r#"{"type":"object","required":["name"],"properties":{
        "name":{"type":"string"},
        "status":{"type":"string","enum":["active","archived"]},
        "tags":{"type":"array","items":{"type":"string"}}}}"#;

    fn schema() -> serde_json::Value {
        from_str(SCHEMA).expect("test schema parses")
    }

    #[test]
    fn valid_record_passes() {
        assert!(validate_schema(&schema()).is_ok());
        let record = json!({"name":"x","status":"active","tags":["a","b"]});
        assert_eq!(validate_record(&record, &schema()), Ok(()));
    }

    #[test]
    fn schema_missing_type_rejected_early() {
        let err = validate_schema(&json!({"properties":{"a":{"type":"string"}}}))
            .expect_err("schema without `type` must be rejected");
        assert!(matches!(err, SchemaError::InvalidSchema { .. }));
    }

    #[test]
    fn missing_required_names_the_field() {
        let err = validate_record(&json!({"status":"active"}), &schema())
            .expect_err("missing required field must fail");
        assert_eq!(
            err,
            SchemaError::MissingRequired {
                path: "$".to_string(),
                field: "name".to_string()
            }
        );
    }

    #[test]
    fn type_mismatch_reports_path_expected_actual() {
        let err =
            validate_record(&json!({"name":42}), &schema()).expect_err("wrong type must fail");
        assert_eq!(
            err,
            SchemaError::TypeMismatch {
                path: "$.name".to_string(),
                expected: "string".to_string(),
                actual: "number".to_string()
            }
        );
    }

    #[test]
    fn value_not_in_enum_fails() {
        let err = validate_record(&json!({"name":"x","status":"bogus"}), &schema())
            .expect_err("non-enum value must fail");
        assert_eq!(
            err,
            SchemaError::NotInEnum {
                path: "$.status".to_string()
            }
        );
    }

    #[test]
    fn array_item_type_mismatch_reports_index() {
        let err = validate_record(&json!({"name":"x","tags":["ok",3]}), &schema())
            .expect_err("bad array item must fail");
        assert_eq!(
            err,
            SchemaError::ItemsTypeMismatch {
                path: "$.tags".to_string(),
                idx: 1
            }
        );
    }
}
