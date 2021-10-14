use crate::spec::transform::aggregate::{AggregateTransformSpec, AggregateOp as AggregateOpSpec};
use crate::proto::gen::transforms::{Aggregate, AggregateOp};

impl Aggregate {
    pub fn new(transform: &AggregateTransformSpec) -> Self {
        let fields: Vec<_> = transform
            .fields
            .iter()
            .map(|f| f.as_ref().map(|f| f.field()).unwrap_or_default())
            .collect();

        let groupby: Vec<_> = transform.groupby.iter().map(|f| f.field()).collect();

        // Initialize aliases with those potentially provided in field objects
        // (e.g. {"field": "foo", "as": "bar"}
        let mut aliases: Vec<_> = transform
            .fields
            .iter()
            .map(|f| f.as_ref().and_then(|f| f.as_()).unwrap_or_default())
            .collect();

        // Overwrite aliases with those provided in the as_ prop of the transform
        for (i, as_) in transform.as_.clone().unwrap_or_default().iter().enumerate() {
            if as_.is_some() {
                aliases[i] = as_.clone().unwrap();
            }
        }

        let ops: Vec<_> = transform.ops.iter().map(|op| {
            match op {
                AggregateOpSpec::Count => {
                    AggregateOp::Count as i32
                }
                AggregateOpSpec::Valid => {
                    AggregateOp::Valid as i32
                }
                AggregateOpSpec::Missing => {
                    AggregateOp::Missing as i32
                }
                AggregateOpSpec::Distinct => {
                    AggregateOp::Distinct as i32
                }
                AggregateOpSpec::Sum => {
                    AggregateOp::Sum as i32
                }
                AggregateOpSpec::Product => {
                    AggregateOp::Product as i32
                }
                AggregateOpSpec::Mean => {
                    AggregateOp::Mean as i32
                }
                AggregateOpSpec::Average => {
                    AggregateOp::Average as i32
                }
                AggregateOpSpec::Variance => {
                    AggregateOp::Variance as i32
                }
                AggregateOpSpec::Variancp => {
                    AggregateOp::Variancp as i32
                }
                AggregateOpSpec::Stdev => {
                    AggregateOp::Stdev as i32
                }
                AggregateOpSpec::Stdevp => {
                    AggregateOp::Stdevp as i32
                }
                AggregateOpSpec::Stderr => {
                    AggregateOp::Stderr as i32
                }
                AggregateOpSpec::Median => {
                    AggregateOp::Median as i32
                }
                AggregateOpSpec::Q1 => {
                    AggregateOp::Q1 as i32
                }
                AggregateOpSpec::Q3 => {
                    AggregateOp::Q3 as i32
                }
                AggregateOpSpec::Ci0 => {
                    AggregateOp::Ci0 as i32
                }
                AggregateOpSpec::Ci1 => {
                    AggregateOp::Ci1 as i32
                }
                AggregateOpSpec::Min => {
                    AggregateOp::Min as i32
                }
                AggregateOpSpec::Max => {
                    AggregateOp::Max as i32
                }
                AggregateOpSpec::Argmin => {
                    AggregateOp::Argmin as i32
                }
                AggregateOpSpec::Argmax => {
                    AggregateOp::Argmax as i32
                }
                AggregateOpSpec::Values => {
                    AggregateOp::Values as i32
                }
            }
        }).collect();

        Self {
            groupby,
            fields,
            ops,
            aliases,
        }
    }
}
