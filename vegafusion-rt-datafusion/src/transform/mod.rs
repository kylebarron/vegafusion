pub mod aggregate;
pub mod bin;
pub mod collect;
pub mod extent;
pub mod filter;
pub mod fold;
pub mod formula;
pub mod identifier;
pub mod impute;
pub mod joinaggregate;
pub mod pipeline;
pub mod pivot;
pub mod project;
pub mod stack;
pub mod timeunit;
pub mod utils;
pub mod window;

use crate::expression::compiler::config::CompilationConfig;

use std::sync::Arc;
use vegafusion_core::error::Result;

use crate::sql::dataframe::SqlDataFrame;
use async_trait::async_trait;
use vegafusion_core::proto::gen::transforms::transform::TransformKind;
use vegafusion_core::proto::gen::transforms::Transform;
use vegafusion_core::task_graph::task_value::TaskValue;
use vegafusion_core::transform::TransformDependencies;

#[async_trait]
pub trait TransformTrait: TransformDependencies {
    async fn eval(
        &self,
        dataframe: Arc<SqlDataFrame>,
        config: &CompilationConfig,
    ) -> Result<(Arc<SqlDataFrame>, Vec<TaskValue>)>;
}

pub fn to_transform_trait(tx: &TransformKind) -> &dyn TransformTrait {
    match tx {
        TransformKind::Filter(tx) => tx,
        TransformKind::Extent(tx) => tx,
        TransformKind::Formula(tx) => tx,
        TransformKind::Bin(tx) => tx,
        TransformKind::Aggregate(tx) => tx,
        TransformKind::Collect(tx) => tx,
        TransformKind::Timeunit(tx) => tx,
        TransformKind::Joinaggregate(tx) => tx,
        TransformKind::Window(tx) => tx,
        TransformKind::Project(tx) => tx,
        TransformKind::Stack(tx) => tx,
        TransformKind::Impute(tx) => tx,
        TransformKind::Pivot(tx) => tx,
        TransformKind::Identifier(tx) => tx,
        TransformKind::Fold(tx) => tx,
    }
}

#[async_trait]
impl TransformTrait for Transform {
    async fn eval(
        &self,
        sql_df: Arc<SqlDataFrame>,
        config: &CompilationConfig,
    ) -> Result<(Arc<SqlDataFrame>, Vec<TaskValue>)> {
        to_transform_trait(self.transform_kind())
            .eval(sql_df, config)
            .await
    }
}
