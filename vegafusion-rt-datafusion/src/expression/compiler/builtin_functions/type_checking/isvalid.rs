/*
 * VegaFusion
 * Copyright (C) 2022 VegaFusion Technologies LLC
 *
 * This program is distributed under multiple licenses.
 * Please consult the license documentation provided alongside
 * this program the details of the active license.
 */
use datafusion::arrow::array::ArrayRef;
use datafusion::arrow::compute::is_not_null;
use datafusion::arrow::datatypes::DataType;
use datafusion::common::DFSchema;
use datafusion::physical_plan::functions::make_scalar_function;
use datafusion::physical_plan::udf::ScalarUDF;
use datafusion_expr::{Expr, ExprSchemable, ReturnTypeFunction, Signature, Volatility};
use std::sync::Arc;
use vegafusion_core::error::{Result, ResultWithContext, VegaFusionError};

/// `isValid(value)`
///
/// Returns true if value is not null, undefined, or NaN, false otherwise.
///
/// Note: Current implementation does not consider NaN values invalid
///
/// See: https://vega.github.io/vega/docs/expressions/#isValid
pub fn is_valid_fn(args: &[Expr], _schema: &DFSchema) -> Result<Expr> {
    if args.len() == 1 {
        let arg = args[0].clone();
        Ok(Expr::IsNotNull(Box::new(arg)))
    } else {
        Err(VegaFusionError::parse(format!(
            "isValid requires a single argument. Received {} arguments",
            args.len()
        )))
    }
}
