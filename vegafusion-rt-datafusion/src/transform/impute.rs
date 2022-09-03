use crate::expression::compiler::config::CompilationConfig;
use crate::sql::compile::order::ToSqlOrderByExpr;
use crate::sql::compile::select::ToSqlSelectItem;
use crate::sql::dataframe::SqlDataFrame;
use crate::transform::TransformTrait;
use async_trait::async_trait;
use datafusion::common::ScalarValue;
use datafusion::dataframe::DataFrame;
use datafusion_expr::logical_plan::JoinType;
use datafusion_expr::{col, lit, when, BuiltInWindowFunction, Expr, WindowFunction};
use sqlgen::dialect::DialectDisplay;
use std::sync::Arc;
use vegafusion_core::arrow::datatypes::DataType;
use vegafusion_core::data::scalar::ScalarValueHelpers;
use vegafusion_core::error::{Result, VegaFusionError};
use vegafusion_core::proto::gen::transforms::Impute;
use vegafusion_core::task_graph::task_value::TaskValue;

#[async_trait]
impl TransformTrait for Impute {
    async fn eval(
        &self,
        dataframe: Arc<SqlDataFrame>,
        _config: &CompilationConfig,
    ) -> Result<(Arc<SqlDataFrame>, Vec<TaskValue>)> {
        // Create ScalarValue used to fill in null values
        let json_value: serde_json::Value =
            serde_json::from_str(self.value_json.as_ref().unwrap())?;

        // JSON numbers are always interpreted as floats, but if the value is an integer we'd
        // like the fill value to be an integer as well to avoid converting an integer input
        // column to floats
        let value = if json_value.is_i64() {
            ScalarValue::from(json_value.as_i64().unwrap())
        } else if json_value.is_f64() && json_value.as_f64().unwrap().fract() == 0.0 {
            ScalarValue::from(json_value.as_f64().unwrap() as i64)
        } else {
            ScalarValue::from_json(&json_value)?
        };

        let dataframe = match self.groupby.len() {
            0 => zero_groupby_sql(self, dataframe, value)?,
            1 => single_groupby_sql(self, dataframe, value)?,
            _ => {
                return Err(VegaFusionError::internal(
                    "Expected zero or one groupby columns to impute",
                ))
            }
        };

        Ok((dataframe, Vec::new()))
    }
}

fn zero_groupby_sql(
    tx: &Impute,
    dataframe: Arc<SqlDataFrame>,
    value: ScalarValue,
) -> Result<Arc<SqlDataFrame>> {
    // Value replacement for field with no groupby fields specified is equivalent to replacing
    // null values of that column with the fill value
    let select_columns: Vec<_> = dataframe
        .schema_df()
        .fields()
        .iter()
        .map(|field| {
            let col_name = field.name();
            if col_name == &tx.field {
                when(col(&tx.field).is_not_null(), col(&tx.field))
                    .otherwise(lit(value.clone()))
                    .unwrap()
                    .alias(&tx.field)
            } else {
                col(col_name)
            }
        })
        .collect();

    dataframe.select(select_columns)
}

fn single_groupby_sql(
    tx: &Impute,
    dataframe: Arc<SqlDataFrame>,
    value: ScalarValue,
) -> Result<Arc<SqlDataFrame>> {
    // Save off names of columns in the original input DataFrame
    let original_columns: Vec<_> = dataframe
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();

    // First step is to build up a new DataFrame that contains the all possible combinations
    // of the `key` and `groupby` columns

    // We're only supporting a single groupby column for now
    let groupby = tx.groupby.get(0).unwrap().clone();

    let key_col = col(&tx.key);
    let key_col_str = key_col.to_sql_select()?.sql(dataframe.dialect())?;

    let group_col = col(&groupby);
    let group_col_str = group_col.to_sql_select()?.sql(dataframe.dialect())?;

    // Build row number expr to apply to input table
    let row_number_expr = Expr::WindowFunction {
        fun: WindowFunction::BuiltInWindowFunction(BuiltInWindowFunction::RowNumber),
        args: Vec::new(),
        partition_by: Vec::new(),
        order_by: Vec::new(),
        window_frame: None,
    }
    .alias("__row_number");
    let row_number_expr_str = row_number_expr.to_sql_select()?.sql(dataframe.dialect())?;

    // Build order by
    let order_by_expr = Expr::Sort {
        expr: Box::new(col("__row_number")),
        asc: true,
        nulls_first: false,
    };
    let order_by_expr_str = order_by_expr.to_sql_order()?.sql(dataframe.dialect())?;

    // Build final selection
    // Finally, select all of the original DataFrame columns, filling in missing values
    // of the `field` columns
    let mut select_columns: Vec<_> = original_columns
        .iter()
        .map(|col_name| {
            if col_name == &tx.field {
                when(col(&tx.field).is_not_null(), col(&tx.field))
                    .otherwise(lit(value.clone()))
                    .unwrap()
                    .alias(&tx.field)
            } else {
                col(col_name)
            }
        })
        .collect();

    // Add undocumented "_impute" column that Vega adds
    select_columns.push(
        when(
            col(&tx.field).is_not_null(),
            Expr::Cast {
                expr: Box::new(Expr::Literal(ScalarValue::Boolean(None))),
                data_type: DataType::Boolean,
            },
        )
        .otherwise(lit(true))
        .unwrap()
        .alias("_impute"),
    );
    let select_column_strs = select_columns
        .iter()
        .map(|c| Ok(c.to_sql_select()?.sql(dataframe.dialect())?))
        .collect::<Result<Vec<_>>>()?;

    let select_column_csv = select_column_strs.join(", ");

    let dataframe = dataframe.chain_query_str(&format!(
        "SELECT {select_column_csv} from (SELECT DISTINCT {key} from {parent} WHERE {key} IS NOT NULL) AS _key \
         CROSS JOIN (SELECT DISTINCT {group} from {parent} WHERE {group} IS NOT NULL) AS _group  \
         LEFT OUTER JOIN (SELECT *, {row_number_expr_str} from {parent}) AS _inner USING ({key}, {group}) \
         ORDER BY {order_by_expr_str}",
        select_column_csv = select_column_csv,
        key = key_col_str,
        group = group_col_str,
        row_number_expr_str = row_number_expr_str,
        order_by_expr_str = order_by_expr_str,
        parent = dataframe.parent_name(),
    ))?;

    Ok(dataframe)
}

fn zero_groupby(
    tx: &Impute,
    dataframe: Arc<DataFrame>,
    value: ScalarValue,
) -> Result<Arc<DataFrame>> {
    // Value replacement for field with no groupby fields specified is equivalent to replacing
    // null values of that column with the fill value
    let select_columns: Vec<_> = dataframe
        .schema()
        .fields()
        .iter()
        .map(|field| {
            let col_name = field.name();
            if col_name == &tx.field {
                when(col(&tx.field).is_not_null(), col(&tx.field))
                    .otherwise(lit(value.clone()))
                    .unwrap()
                    .alias(&tx.field)
            } else {
                col(col_name)
            }
        })
        .collect();

    Ok(dataframe.select(select_columns)?)
}

fn single_groupby(
    tx: &Impute,
    dataframe: Arc<DataFrame>,
    value: ScalarValue,
) -> Result<Arc<DataFrame>> {
    // Save off names of columns in the original input DataFrame
    let original_columns: Vec<_> = dataframe
        .schema()
        .fields()
        .iter()
        .map(|field| field.name().clone())
        .collect();

    // First step is to build up a new DataFrame that contains the all possible combinations
    // of the `key` and `groupby` columns

    // We're only supporting a single groupby column for now
    let groupby = tx.groupby.get(0).unwrap().clone();

    // Make separate dataframes containing all unique values of the `key` and `groupby` columns
    let key_df = dataframe.aggregate(vec![col(&tx.key)], Vec::new())?;
    let groupby_df = dataframe.aggregate(vec![col(&groupby)], Vec::new())?;

    // DataFusion doesn't yet expose the cross join operation through the DataFrame
    // API, so for now we implement the cross join by adding dummy constant values columns
    // to each
    let key_df = key_df.select(vec![Expr::Wildcard, lit(true).alias("__true_key")])?;
    let groupby_df = groupby_df.select(vec![Expr::Wildcard, lit(true).alias("__true_groupby")])?;
    let all_combos_df = key_df
        .join(
            groupby_df,
            JoinType::Inner,
            &["__true_key"],
            &["__true_groupby"],
            None,
        )?
        .select_columns(&[&tx.key, &groupby])?;

    // Next we take the input DataFrame and
    //  1) Rename the key and groupby columns to avoid collision on join
    //  2) Add a __row_number column that we can sort by at the end to preserver the input
    //     row order
    let mut select_columns: Vec<_> = dataframe
        .schema()
        .fields()
        .iter()
        .map(|field| {
            if field.name() == &tx.key {
                col(field.name()).alias("__key")
            } else if field.name() == &groupby {
                col(field.name()).alias("__groupby")
            } else {
                col(field.name())
            }
        })
        .collect();

    let row_number_expr = Expr::WindowFunction {
        fun: WindowFunction::BuiltInWindowFunction(BuiltInWindowFunction::RowNumber),
        args: Vec::new(),
        partition_by: Vec::new(),
        order_by: Vec::new(),
        window_frame: None,
    }
    .alias("__row_number");

    select_columns.push(row_number_expr);

    let dataframe = dataframe.select(select_columns)?;

    // Now join dataframe on key and groupby columns. Use a left outer join to introduce new
    // rows for combinations of groupby and key that were not originally present.
    // Also sort by __row_number to restore the original ordering of the input DataFrame with
    // null values (which will be replaced below) are pushed to the end.
    let joined = all_combos_df
        .join(
            dataframe,
            JoinType::Left,
            &[&tx.key, &groupby],
            &["__key", "__groupby"],
            None,
        )?
        .sort(vec![Expr::Sort {
            expr: Box::new(col("__row_number")),
            asc: true,
            nulls_first: false,
        }])?;

    // Finally, select all of the original DataFrame columns, filling in missing values
    // of the `field` columns
    let mut select_columns: Vec<_> = original_columns
        .iter()
        .map(|col_name| {
            if col_name == &tx.field {
                when(col(&tx.field).is_not_null(), col(&tx.field))
                    .otherwise(lit(value.clone()))
                    .unwrap()
                    .alias(&tx.field)
            } else {
                col(col_name)
            }
        })
        .collect();

    // Add undocumented "_impute" column that Vega adds
    select_columns.push(
        when(
            col(&tx.field).is_not_null(),
            Expr::Literal(ScalarValue::Boolean(None)),
        )
        .otherwise(lit(true))
        .unwrap()
        .alias("_impute"),
    );

    let dataframe = joined.select(select_columns)?;
    Ok(dataframe)
}
