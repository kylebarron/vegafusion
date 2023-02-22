use crate::udfs::datetime::to_utc_timestamp::to_timestamp_ms;
use chrono::NaiveDateTime;
use std::sync::Arc;
use vegafusion_common::{
    arrow::{
        array::{ArrayRef, StringArray, TimestampMillisecondArray},
        datatypes::{DataType, TimeUnit},
    },
    datafusion_common::{DataFusionError, ScalarValue},
    datafusion_expr::{
        ColumnarValue, ReturnTypeFunction, ScalarFunctionImplementation, ScalarUDF, Signature,
        TypeSignature, Volatility,
    },
};

fn make_time_format_udf() -> ScalarUDF {
    let time_fn: ScalarFunctionImplementation = Arc::new(move |args: &[ColumnarValue]| {
        // Argument order
        // [0] data array
        let data_array = match &args[0] {
            ColumnarValue::Array(array) => array.clone(),
            ColumnarValue::Scalar(scalar) => scalar.to_array(),
        };

        // [1] time format string
        let format_str = if let ColumnarValue::Scalar(format_str) = &args[1] {
            format_str.to_string()
        } else {
            return Err(DataFusionError::Internal(
                "Expected output_tz to be a scalar".to_string(),
            ));
        };

        if matches!(data_array.data_type(), DataType::Null) {
            return Ok(ColumnarValue::Array(data_array));
        }

        let data_array = to_timestamp_ms(&data_array)?;

        let utc_millis_array = data_array
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .unwrap();

        let formatted = Arc::new(StringArray::from_iter(utc_millis_array.iter().map(
            |utc_millis| {
                utc_millis.map(|utc_millis| {
                    // Load as UTC datetime
                    let utc_seconds = utc_millis / 1_000;
                    let utc_nanos = (utc_millis % 1_000 * 1_000_000) as u32;
                    let naive_datetime = NaiveDateTime::from_timestamp_opt(utc_seconds, utc_nanos)
                        .expect("invalid or out-of-range datetime");

                    // Format as string
                    let formatted = naive_datetime.format(&format_str);
                    formatted.to_string()
                })
            },
        ))) as ArrayRef;

        // maybe back to scalar
        if formatted.len() != 1 {
            Ok(ColumnarValue::Array(formatted))
        } else {
            ScalarValue::try_from_array(&formatted, 0).map(ColumnarValue::Scalar)
        }
    });

    let return_type: ReturnTypeFunction = Arc::new(move |_| Ok(Arc::new(DataType::Utf8)));

    let signature: Signature = Signature::one_of(
        vec![
            TypeSignature::Exact(vec![
                DataType::Timestamp(TimeUnit::Millisecond, None),
                DataType::Utf8,
            ]),
            TypeSignature::Exact(vec![
                DataType::Timestamp(TimeUnit::Nanosecond, None),
                DataType::Utf8,
            ]),
        ],
        Volatility::Immutable,
    );

    ScalarUDF::new("format_timestamp", &signature, &return_type, &time_fn)
}

lazy_static! {
    pub static ref FORMAT_TIMESTAMP_UDF: ScalarUDF = make_time_format_udf();
}
