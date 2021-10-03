use crate::error::{Result, VegaFusionError};
use crate::expression::ast::base::Expression;
use crate::expression::ast::identifier::Identifier;
use crate::expression::ast::member::MemberExpression;
use crate::expression::compiler::compile;
use crate::expression::compiler::config::CompilationConfig;
use crate::expression::compiler::utils::{data_type, is_numeric_datatype, ExprHelpers};
use datafusion::arrow::array::{
    new_null_array, Array, ArrayRef, Int32Array, Int64Array, ListArray, StructArray,
};
use datafusion::arrow::compute::{cast, kernels};
use datafusion::arrow::datatypes::DataType;
use datafusion::error::DataFusionError;
use datafusion::logical_plan::{col, DFSchema, Expr};
use datafusion::physical_plan::functions::{
    make_scalar_function, ReturnTypeFunction, ScalarFunctionImplementation, Signature,
};
use datafusion::physical_plan::udf::ScalarUDF;
use datafusion::physical_plan::ColumnarValue;
use datafusion::scalar::ScalarValue;
use std::convert::TryFrom;
use std::sync::Arc;

pub fn compile_member(
    node: &MemberExpression,
    config: &CompilationConfig,
    schema: &DFSchema,
) -> Result<Expr> {
    // Maybe an numeric array index
    let mut index: Option<usize> = None;

    // Get string-form of index
    let property_string = if node.computed {
        // e.g. foo[val]
        let compiled_property = compile(&node.property, config, Some(schema))?;
        let evaluated_property = compiled_property.eval_to_scalar()?;
        let prop_str = evaluated_property.to_string();
        if is_numeric_datatype(&evaluated_property.get_datatype()) {
            let int_array = cast(&evaluated_property.to_array(), &DataType::Int64).unwrap();
            let int_array = int_array.as_any().downcast_ref::<Int64Array>().unwrap();
            index = Some(int_array.value(0) as usize);
        } else {
            // Try to convert string to number
            if let Ok(v) = prop_str.parse::<f64>() {
                // Then case to usize
                index = Some(v as usize);
            }
        }
        prop_str
    } else if let Expression::Identifier(property) = node.property.as_ref() {
        // e.g. foo.val
        property.name.clone()
    } else {
        return Err(VegaFusionError::compilation_error(&format!(
            "Invalid membership property: {}",
            node.property
        )));
    };

    // Handle datum property access. These represent DataFusion column expressions

    match node.object.as_ref() {
        Expression::Identifier(Identifier { name, .. }) if name == "datum" => {
            let col_expr = col(&property_string);
            return Ok(col_expr);
        }
        _ => {}
    }

    let compiled_object = compile(&node.object, config, Some(schema))?;
    let dtype = data_type(&compiled_object, schema)?;

    let udf = match dtype {
        DataType::Struct(_) => make_get_object_member_udf(&dtype, &property_string)?,
        _ => {
            if property_string == "length" {
                // Special case to treat foo.length as length(foo) when foo is not an object
                // make_length()
                todo!("Special case to treat foo.length as length(foo) when foo is not an object")
            } else if matches!(
                dtype,
                DataType::List(_)
                    | DataType::FixedSizeList(_, _)
                    | DataType::Utf8
                    | DataType::LargeUtf8
            ) {
                if let Some(index) = index {
                    make_get_element_udf(index as i32)
                } else {
                    return Err(VegaFusionError::compilation_error(&format!(
                        "Non-numeric element index: {}",
                        property_string
                    )));
                }
            } else {
                return Err(VegaFusionError::compilation_error(&format!(
                    "Invalid target for member access: {}",
                    node.object
                )));
            }
        }
    };

    Ok(Expr::ScalarUDF {
        fun: Arc::new(udf),
        args: vec![compiled_object],
    })
}

pub fn make_get_object_member_udf(
    object_type: &DataType,
    property_name: &str,
) -> Result<ScalarUDF> {
    let (field_index, field_type) = if let DataType::Struct(fields) = object_type {
        match fields
            .iter()
            .enumerate()
            .find(|(_i, f)| f.name() == &property_name)
        {
            Some((field_index, field)) => (field_index, field.data_type().clone()),
            None => {
                return Err(VegaFusionError::compilation_error(&format!(
                    "No object property named {}",
                    property_name
                )))
            }
        }
    } else {
        return Err(VegaFusionError::compilation_error(
            "Target of object property access is not a Struct type",
        ));
    };

    let get = move |args: &[ArrayRef]| {
        // Signature ensures there is a single argument
        let object = &args[0];

        let struct_array = object.as_any().downcast_ref::<StructArray>().unwrap();
        Ok(struct_array.column(field_index).clone())
    };
    let get = make_scalar_function(get);

    let return_type: ReturnTypeFunction =
        Arc::new(move |_dtype: &[DataType]| Ok(Arc::new(field_type.clone())));

    Ok(ScalarUDF::new(
        &format!("get[{}]", property_name),
        &Signature::Exact(vec![object_type.clone()]),
        &return_type,
        &get,
    ))
}

pub fn make_get_element_udf(index: i32) -> ScalarUDF {
    let get_fn: ScalarFunctionImplementation = Arc::new(move |args: &[ColumnarValue]| {
        // Signature ensures there is a single argument
        let arg = &args[0];
        Ok(match arg {
            ColumnarValue::Scalar(value) => {
                match value {
                    ScalarValue::List(Some(arr), element_dtype) => {
                        match arr.get(index as usize) {
                            Some(element) => {
                                // Scalar element of list
                                ColumnarValue::Scalar(element.clone())
                            }
                            None => {
                                // out of bounds, null
                                ColumnarValue::Scalar(
                                    ScalarValue::try_from(element_dtype.as_ref()).unwrap(),
                                )
                            }
                        }
                    }
                    ScalarValue::Utf8(Some(s)) | ScalarValue::LargeUtf8(Some(s)) => {
                        match s.get((index as usize)..((index + 1) as usize)) {
                            Some(substring) => ColumnarValue::Scalar(ScalarValue::from(substring)),
                            None => {
                                // out of bounds, null
                                ColumnarValue::Scalar(
                                    ScalarValue::try_from(&DataType::Utf8).unwrap(),
                                )
                            }
                        }
                    }
                    _ => {
                        // null
                        ColumnarValue::Scalar(ScalarValue::try_from(&DataType::Float64).unwrap())
                    }
                }
            }
            ColumnarValue::Array(array) => {
                match array.data_type() {
                    DataType::Utf8 | DataType::LargeUtf8 => {
                        // String length
                        ColumnarValue::Array(
                            kernels::substring::substring(array.as_ref(), index as i64, &Some(1))
                                .unwrap(),
                        )
                    }
                    DataType::List(_) => {
                        // There is not substring-like kernel for general list arrays.
                        // So instead, build indices into the flat buffer and use take
                        let array = array.as_any().downcast_ref::<ListArray>().unwrap();
                        let mut take_index_builder = Int32Array::builder(array.len());
                        let offsets = array.value_offsets();
                        let _flat_values = array.values();

                        for i in 0..array.len() {
                            let el_start = offsets[i];
                            let el_stop = offsets[i + 1];
                            if el_start + index < el_stop {
                                take_index_builder.append_value(el_start + index).unwrap();
                            } else {
                                take_index_builder.append_null().unwrap();
                            }
                        }

                        let result = kernels::take::take(
                            array.values().as_ref(),
                            &take_index_builder.finish(),
                            Default::default(),
                        )
                        .unwrap();

                        ColumnarValue::Array(result)
                    }
                    _ => ColumnarValue::Array(new_null_array(&DataType::Float64, array.len())),
                }
            }
        })
    });

    let return_type: ReturnTypeFunction = Arc::new(move |dtype| {
        let new_dtype = match &dtype[0] {
            DataType::Utf8 => DataType::Utf8,
            DataType::List(field) => field.data_type().clone(),
            dtype => {
                return Err(DataFusionError::Internal(format!(
                    "Unsupported datatype for get index {:?}",
                    dtype
                )))
            }
        };
        Ok(Arc::new(new_dtype))
    });
    ScalarUDF::new(
        &format!("get[{}]", index),
        &Signature::Any(1),
        &return_type,
        &get_fn,
    )
}
