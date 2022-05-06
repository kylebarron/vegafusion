/*
 * VegaFusion
 * Copyright (C) 2022 VegaFusion Technologies LLC
 *
 * This program is distributed under multiple licenses.
 * Please consult the license documentation provided alongside
 * this program the details of the active license.
 */
use crate::error::Result;
use crate::planning::stitch::CommPlan;
use crate::proto::gen::tasks::{Variable, VariableNamespace};
use crate::spec::chart::{ChartSpec, ChartVisitor, MutChartVisitor};
use crate::spec::data::DataSpec;
use crate::spec::mark::{MarkEncodingField, MarkSpec};
use crate::spec::scale::{ScaleSpec, ScaleTypeSpec};
use crate::spec::transform::formula::FormulaTransformSpec;
use crate::spec::transform::TransformSpec;
use crate::task_graph::graph::ScopedVariable;
use crate::task_graph::scope::TaskScope;
use itertools::sorted;
use std::collections::{HashMap, HashSet};

/// This planning phase converts select datetime columns from the default millisecond UTC
/// representation to naive datetime strings in the local timezone. This is only done for datetime
/// columns that are scaled using a (non-utc) `time` scale in the client specification.
///
/// This is needed in order for the chart displayed by the client to be consistent regardless of
/// the browser's local timezone.  Viewers from all timezones should see the chart displayed as
/// it would look when generated by pure Vega in the `local_tz` timezone.
pub fn stringify_local_datetimes(
    server_spec: &mut ChartSpec,
    client_spec: &mut ChartSpec,
    comm_plan: &CommPlan,
) -> Result<()> {
    // Build task scope for client spec
    let client_scope = client_spec.to_task_scope()?;

    // Collect the name/scope of all time scales
    let mut visitor = CollectTimeScalesVisitor::new();
    client_spec.walk(&mut visitor)?;
    let local_time_scales = visitor.local_time_scales;

    // Gather candidate datasets
    let candidate_datasets: HashSet<_> = comm_plan
        .server_to_client
        .iter()
        .cloned()
        .filter(|var| var.0.namespace == VariableNamespace::Data as i32)
        .collect();

    // Collect data fields to convert to datetime strings
    let mut visitor = CollectLocalTimeScaledFieldsVisitor::new(
        client_scope,
        local_time_scales,
        candidate_datasets,
    );
    client_spec.walk(&mut visitor)?;
    let local_datetime_fields = visitor.local_datetime_fields;

    // Add formula transforms to server spec
    let server_scope = server_spec.to_task_scope()?;
    let mut visitor =
        StringifyLocalDatetimeFieldsVisitor::new(local_datetime_fields.clone(), server_scope);
    server_spec.walk_mut(&mut visitor)?;

    // Add format spec to client spec (to parse strings as local dates)
    let mut visitor = FormatLocalDatetimeFieldsVisitor::new(local_datetime_fields);
    client_spec.walk_mut(&mut visitor);

    Ok(())
}

/// Visitor to collect the non-UTC time scales
struct CollectTimeScalesVisitor {
    pub local_time_scales: HashSet<ScopedVariable>,
}

impl CollectTimeScalesVisitor {
    pub fn new() -> Self {
        Self {
            local_time_scales: Default::default(),
        }
    }
}

impl ChartVisitor for CollectTimeScalesVisitor {
    fn visit_scale(&mut self, scale: &ScaleSpec, scope: &[u32]) -> Result<()> {
        if matches!(scale.type_, Some(ScaleTypeSpec::Time)) {
            self.local_time_scales
                .insert((Variable::new_scale(&scale.name), Vec::from(scope)));
        }

        Ok(())
    }
}

/// Visitor to collect data fields that are scaled by a non-UTC time scale
struct CollectLocalTimeScaledFieldsVisitor {
    pub scope: TaskScope,
    pub candidate_datasets: HashSet<ScopedVariable>,
    pub local_time_scales: HashSet<ScopedVariable>,
    pub local_datetime_fields: HashMap<ScopedVariable, HashSet<String>>,
}

impl CollectLocalTimeScaledFieldsVisitor {
    pub fn new(
        scope: TaskScope,
        local_time_scales: HashSet<ScopedVariable>,
        candidate_datasets: HashSet<ScopedVariable>,
    ) -> Self {
        Self {
            scope,
            candidate_datasets,
            local_time_scales,
            local_datetime_fields: Default::default(),
        }
    }
}

impl ChartVisitor for CollectLocalTimeScaledFieldsVisitor {
    fn visit_non_group_mark(&mut self, mark: &MarkSpec, scope: &[u32]) -> Result<()> {
        if let Some(mark_from) = &mark.from {
            if let Some(mark_data) = &mark_from.data {
                let data_var = Variable::new_data(mark_data);
                let resolved_data = self.scope.resolve_scope(&data_var, scope)?;
                let resolved_data_scoped = (resolved_data.var.clone(), resolved_data.scope);
                if self.candidate_datasets.contains(&resolved_data_scoped) {
                    // We've found a mark with a dataset that is eligible for date string
                    // conversion
                    if let Some(encode) = &mark.encode {
                        for (_, encodings) in encode.encodings.iter() {
                            for (_, channels) in encodings.channels.iter() {
                                for channel in channels.to_vec() {
                                    if let (Some(scale), Some(MarkEncodingField::Field(field))) =
                                        (&channel.scale, &channel.field)
                                    {
                                        let scale_var = Variable::new_scale(scale);
                                        let resolved_scale =
                                            self.scope.resolve_scope(&scale_var, scope)?;
                                        let resolved_scoped_scale = (
                                            resolved_scale.var.clone(),
                                            resolved_scale.scope.clone(),
                                        );

                                        if self.local_time_scales.contains(&resolved_scoped_scale) {
                                            // Save off field for dataset
                                            let entry = self
                                                .local_datetime_fields
                                                .entry(resolved_data_scoped.clone());
                                            let fields = entry.or_default();
                                            fields.insert(field.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Visitor to stringify select datetime fields
struct StringifyLocalDatetimeFieldsVisitor {
    pub local_datetime_fields: HashMap<ScopedVariable, HashSet<String>>,
    pub scope: TaskScope,
}

impl StringifyLocalDatetimeFieldsVisitor {
    pub fn new(
        local_datetime_fields: HashMap<ScopedVariable, HashSet<String>>,
        scope: TaskScope,
    ) -> Self {
        Self {
            local_datetime_fields,
            scope,
        }
    }
}

impl MutChartVisitor for StringifyLocalDatetimeFieldsVisitor {
    fn visit_data(&mut self, data: &mut DataSpec, scope: &[u32]) -> Result<()> {
        let data_var = (Variable::new_data(&data.name), Vec::from(scope));
        if let Some(fields) = self.local_datetime_fields.get(&data_var) {
            for field in sorted(fields) {
                let transforms = &mut data.transform;
                let transform = FormulaTransformSpec {
                    expr: format!("timeFormat(datum['{}'], '%Y-%m-%d %H:%M:%S.%L')", field),
                    as_: field.to_string(),
                    extra: Default::default(),
                };
                transforms.push(TransformSpec::Formula(transform))
            }
        }

        // Check if dataset is a child a stringified dataset. If so, we need to convert
        // datetime strings back to the utc millisecond representation
        if let Some(source) = &data.source {
            let source_var = Variable::new_data(source);
            let source_resolved = self.scope.resolve_scope(&source_var, scope)?;
            let source_resolved_var = (source_resolved.var, source_resolved.scope);
            if let Some(fields) = self.local_datetime_fields.get(&source_resolved_var) {
                for field in sorted(fields) {
                    let transforms = &mut data.transform;
                    let transform = FormulaTransformSpec {
                        expr: format!("toDate(datum['{}'])", field),
                        as_: field.to_string(),
                        extra: Default::default(),
                    };
                    transforms.insert(0, TransformSpec::Formula(transform))
                }
            }
        }

        Ok(())
    }
}

/// Visitor to add format parse specification for local dates
struct FormatLocalDatetimeFieldsVisitor {
    pub local_datetime_fields: HashMap<ScopedVariable, HashSet<String>>,
}

impl FormatLocalDatetimeFieldsVisitor {
    pub fn new(local_datetime_fields: HashMap<ScopedVariable, HashSet<String>>) -> Self {
        Self {
            local_datetime_fields,
        }
    }
}

impl MutChartVisitor for FormatLocalDatetimeFieldsVisitor {
    fn visit_data(&mut self, data: &mut DataSpec, scope: &[u32]) -> Result<()> {
        let data_var = (Variable::new_data(&data.name), Vec::from(scope));
        if let Some(fields) = self.local_datetime_fields.get(&data_var) {
            for field in sorted(fields) {
                let transforms = &mut data.transform;
                let transform = FormulaTransformSpec {
                    expr: format!("toDate(datum['{}'])", field),
                    as_: field.to_string(),
                    extra: Default::default(),
                };
                transforms.insert(0, TransformSpec::Formula(transform))
            }
        }

        Ok(())
    }
}
