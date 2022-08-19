/*
 * VegaFusion
 * Copyright (C) 2022 VegaFusion Technologies LLC
 *
 * This program is distributed under multiple licenses.
 * Please consult the license documentation provided alongside
 * this program the details of the active license.
 */
use crate::expression::compiler::compile;
use crate::expression::compiler::config::CompilationConfig;
use crate::expression::compiler::utils::{to_numeric, ExprHelpers};
use crate::transform::TransformTrait;
use async_trait::async_trait;
use datafusion::dataframe::DataFrame;
use datafusion::logical_plan::{col, lit, DFSchema};
use datafusion::physical_plan::functions::make_scalar_function;
use datafusion::physical_plan::udf::ScalarUDF;
use datafusion::scalar::ScalarValue;
use datafusion_expr::{abs, floor, round, when, Expr, ReturnTypeFunction, Signature, Volatility};
use float_cmp::approx_eq;
use std::ops::{Add, Div, Mul, Sub};
use std::sync::Arc;
use vegafusion_core::arrow::array::{ArrayRef, Float64Array, Int64Array};
use vegafusion_core::arrow::compute::unary;
use vegafusion_core::arrow::datatypes::{DataType, Field};
use vegafusion_core::data::scalar::ScalarValueHelpers;
use vegafusion_core::error::{Result, ResultWithContext, VegaFusionError};

use crate::sql::dataframe::SqlDataFrame;
use vegafusion_core::proto::gen::transforms::Bin;
use vegafusion_core::task_graph::task_value::TaskValue;

#[async_trait]
impl TransformTrait for Bin {
    async fn eval(
        &self,
        dataframe: Arc<DataFrame>,
        config: &CompilationConfig,
    ) -> Result<(Arc<DataFrame>, Vec<TaskValue>)> {
        let schema = dataframe.schema();

        // Compute binning solution
        let params = calculate_bin_params(self, schema, config)?;

        let BinParams {
            start,
            stop,
            step,
            n,
        } = params;
        let bin_starts: Vec<f64> = (0..n).map(|i| start + step * i as f64).collect();
        let last_stop = *bin_starts.last().unwrap() + step;

        // Compute output signal value
        let output_value = compute_output_value(&self, start, stop, step);

        // Investigate: Would it be faster to define this function once and input the binning
        // parameters?
        //
        // Implementation handles Float64 and Int64 separately to avoid having DataFusion
        // copy the full integer array into a float array. This improves performance on integer
        // columns, but this should be extended to the other numeric types as well.
        let bin = move |args: &[ArrayRef]| {
            let arg = &args[0];
            let dtype = arg.data_type();
            let binned_values = match dtype {
                DataType::Float64 => {
                    let field_values = args[0].as_any().downcast_ref::<Float64Array>().unwrap();
                    let binned_values: Float64Array = unary(field_values, |v| {
                        lookup_bin_edge(v, bin_starts.as_slice(), step, last_stop)
                    });
                    binned_values
                }
                DataType::Int64 => {
                    let field_values = args[0].as_any().downcast_ref::<Int64Array>().unwrap();
                    let binned_values: Float64Array = unary(field_values, |v| {
                        let v = v as f64;
                        lookup_bin_edge(v, bin_starts.as_slice(), step, last_stop)
                    });
                    binned_values
                }
                _ => unreachable!(),
            };

            Ok(Arc::new(binned_values) as ArrayRef)
        };
        let bin = make_scalar_function(bin);

        let return_type: ReturnTypeFunction = Arc::new(move |_| Ok(Arc::new(DataType::Float64)));
        let bin = ScalarUDF::new(
            "bin",
            &Signature::uniform(
                1,
                vec![DataType::Float64, DataType::Int64],
                Volatility::Immutable,
            ),
            &return_type,
            &bin,
        );

        let bin_start = bin.call(vec![to_numeric(col(&self.field), dataframe.schema())?]);

        // Name binned columns
        let (bin_start, name) = if let Some(as0) = &self.alias_0 {
            (bin_start.alias(as0), as0.to_string())
        } else {
            (bin_start.alias("bin0"), "bin0".to_string())
        };

        let mut select_exprs: Vec<_> = dataframe
            .schema()
            .fields()
            .iter()
            .filter_map(|field| {
                if field.name() != &name {
                    Some(col(field.name()))
                } else {
                    None
                }
            })
            .collect();
        select_exprs.push(bin_start);
        let dataframe = dataframe
            .select(select_exprs)
            .with_context(|| "Failed to evaluate binning transform".to_string())?;

        // Split end into a separate select so that DataFusion knows to offset from previously
        // computed bin start, rather than recompute it.
        let bin_end = col(&name) + lit(step);
        let (bin_end, name) = if let Some(as1) = &self.alias_1 {
            (bin_end.alias(as1), as1.to_string())
        } else {
            (bin_end.alias("bin1"), "bin1".to_string())
        };

        let mut select_exprs: Vec<_> = dataframe
            .schema()
            .fields()
            .iter()
            .filter_map(|field| {
                if field.name() != &name {
                    Some(col(field.name()))
                } else {
                    None
                }
            })
            .collect();
        select_exprs.push(bin_end);

        let dataframe = dataframe
            .select(select_exprs)
            .with_context(|| "Failed to evaluate binning transform".to_string())?;

        Ok((dataframe, output_value.into_iter().collect()))
    }

    async fn eval_sql(
        &self,
        sql_df: Arc<SqlDataFrame>,
        config: &CompilationConfig,
    ) -> Result<(Arc<SqlDataFrame>, Vec<TaskValue>)> {
        let schema = sql_df.schema_df();

        // Compute binning solution
        let params = calculate_bin_params(self, &schema, config)?;

        let BinParams {
            start,
            stop,
            step,
            n,
        } = params;
        let bin_starts: Vec<f64> = (0..n).map(|i| start + step * i as f64).collect();
        let last_stop = *bin_starts.last().unwrap() + step;

        // Compute output signal value
        let output_value = compute_output_value(&self, start, stop, step);

        // Add column with bin index
        let bin_index_name = "__bin_index";
        let bin_index = floor((col(&self.field).sub(lit(start)).div(lit(step))).add(lit(1.0e-14)))
            .alias(bin_index_name);
        let sql_df = sql_df.select(vec![Expr::Wildcard, bin_index.clone()])?;

        // Add column with bin start
        let bin_start = (col(bin_index_name).mul(lit(step))).add(lit(start));
        let bin_start_name = self.alias_0.clone().unwrap_or("bin0".to_string());

        // Explicitly cast (-)inf to float64 to help DataFusion with type inference
        let inf = Expr::Cast {
            expr: Box::new(lit(f64::INFINITY)),
            data_type: DataType::Float64,
        };
        let neg_inf = Expr::Cast {
            expr: Box::new(lit(f64::NEG_INFINITY)),
            data_type: DataType::Float64,
        };
        let eps = lit(1.0e-14);

        let bin_start = when(col(bin_index_name).lt(lit(0.0)), neg_inf)
            .when(
                abs(col(&self.field).sub(lit(last_stop))).lt(eps),
                lit(*bin_starts.last().unwrap()),
            )
            .when(col(bin_index_name).gt_eq(lit(n)), inf)
            .otherwise(bin_start)?
            .alias(&bin_start_name);

        let sql_df = sql_df.select(vec![Expr::Wildcard, bin_start])?;

        // Add bin end column
        let bin_end_name = self.alias_1.clone().unwrap_or("bin1".to_string());
        let bin_end = (col(&bin_start_name) + lit(step)).alias(&bin_end_name);

        // Compute final projection that removes __bin_index column
        let mut select_exprs = schema
            .fields()
            .iter()
            .filter_map(|field| {
                let name = field.name();
                if name == &bin_start_name || name == &bin_end_name {
                    None
                } else {
                    Some(col(name))
                }
            })
            .collect::<Vec<_>>();
        select_exprs.push(col(&bin_start_name));
        select_exprs.push(bin_end);

        let sql_df = sql_df.select(select_exprs)?;

        Ok((sql_df, output_value.into_iter().collect()))
    }
}

fn compute_output_value(bin_tx: &Bin, start: f64, stop: f64, step: f64) -> Option<TaskValue> {
    let mut fname = bin_tx.field.clone();
    fname.insert_str(0, "bin_");

    let fields = ScalarValue::List(
        Some(vec![ScalarValue::from(bin_tx.field.as_str())]),
        Box::new(Field::new("item", DataType::Utf8, true)),
    );
    let output_value = if bin_tx.signal.is_some() {
        Some(TaskValue::Scalar(ScalarValue::from(vec![
            ("fields", fields),
            ("fname", ScalarValue::from(fname.as_str())),
            ("start", ScalarValue::from(start)),
            ("step", ScalarValue::from(step)),
            ("stop", ScalarValue::from(stop)),
        ])))
    } else {
        None
    };
    output_value
}

#[inline(always)]
fn lookup_bin_edge(v: f64, bin_starts: &[f64], step: f64, last_stop: f64) -> f64 {
    let n = bin_starts.len() as i32;
    let bin_ind = (1.0e-14 + (v - bin_starts[0]) / step).floor() as i32;
    if bin_ind < 0 {
        f64::NEG_INFINITY
    } else if bin_ind == n && (v - last_stop).abs() <= 1.0e-14 {
        *bin_starts.last().unwrap()
    } else if bin_ind >= n {
        f64::INFINITY
    } else {
        bin_starts[bin_ind as usize]
    }
}

#[derive(Clone, Debug)]
pub struct BinParams {
    pub start: f64,
    pub stop: f64,
    pub step: f64,
    pub n: i32,
}

pub fn calculate_bin_params(
    tx: &Bin,
    schema: &DFSchema,
    config: &CompilationConfig,
) -> Result<BinParams> {
    // Evaluate extent
    let extent_expr = compile(tx.extent.as_ref().unwrap(), config, Some(schema))?;
    let extent_scalar = extent_expr.eval_to_scalar()?;

    let extent = extent_scalar.to_f64x2()?;

    let [min_, max_] = extent;
    if min_ > max_ {
        return Err(VegaFusionError::specification(&format!(
            "extent[1] must be greater than extent[0]: Received {:?}",
            extent
        )));
    }

    // Initialize span to default value
    let mut span = if !approx_eq!(f64, min_, max_) {
        max_ - min_
    } else if !approx_eq!(f64, min_, 0.0) {
        min_.abs()
    } else {
        1.0
    };

    // Override span with specified value if available
    if let Some(span_expression) = &tx.span {
        let span_expr = compile(span_expression, config, Some(schema))?;
        let span_scalar = span_expr.eval_to_scalar()?;
        if let Ok(span_f64) = span_scalar.to_f64() {
            span = span_f64;
        }
    }

    let logb = tx.base.ln();

    let step = if let Some(step) = tx.step {
        // Use provided step as-is
        step
    } else if !tx.steps.is_empty() {
        // If steps is provided, limit step to one of the elements.
        // Choose the first element of steps that will result in fewer than maxmins
        let min_step_size = span / tx.maxbins;
        let valid_steps: Vec<_> = tx
            .steps
            .clone()
            .into_iter()
            .filter(|s| *s > min_step_size)
            .collect();
        *valid_steps
            .first()
            .unwrap_or_else(|| tx.steps.last().unwrap())
    } else {
        // Otherwise, use span to determine the step size
        let level = (tx.maxbins.ln() / logb).ceil();
        let minstep = tx.minstep;
        let mut step = minstep.max(tx.base.powf((span.ln() / logb).round() - level));

        // increase step size if too many bins
        while (span / step).ceil() > tx.maxbins {
            step *= tx.base;
        }

        // decrease step size if allowed
        for div in &tx.divide {
            let v = step / div;
            if v >= minstep && span / v <= tx.maxbins {
                step = v
            }
        }
        step
    };

    // Update precision of min_ and max_
    let v = step.ln();
    let precision = if v >= 0.0 {
        0.0
    } else {
        (-v / logb).floor() + 1.0
    };
    let eps = tx.base.powf(-precision - 1.0);
    let (min_, max_) = if tx.nice {
        let v = (min_ / step + eps).floor() * step;
        let min_ = if min_ < v { v - step } else { v };
        let max_ = (max_ / step).ceil() * step;
        (min_, max_)
    } else {
        (min_, max_)
    };

    // Compute start and stop
    let start = min_;
    let stop = if !approx_eq!(f64, max_, min_) {
        max_
    } else {
        min_ + step
    };

    // Handle anchor
    let (start, stop) = if let Some(anchor) = tx.anchor {
        let shift = anchor - (start + step * ((anchor - start) / step).floor());
        (start + shift, stop + shift)
    } else {
        (start, stop)
    };

    Ok(BinParams {
        start,
        stop,
        step,
        n: ((stop - start) / step).ceil() as i32,
    })
}
