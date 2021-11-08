use crate::error::Result;
use crate::planning::data_graph::get_supported_data_variables;
use crate::proto::gen::tasks::Variable;
use crate::spec::chart::{ChartSpec, MutChartVisitor};
use crate::spec::data::{DataSpec, DataSupported};
use crate::spec::mark::MarkSpec;
use crate::task_graph::scope::TaskScope;
use crate::task_graph::task_graph::ScopedVariable;
use std::collections::HashMap;

pub fn extract_server_data(
    client_spec: &mut ChartSpec,
    task_scope: &mut TaskScope,
) -> Result<ChartSpec> {
    let supported_vars = get_supported_data_variables(client_spec)?;

    let mut extract_server_visitor = ExtractServerDataVisitor::new(supported_vars, task_scope);
    client_spec.walk_mut(&mut extract_server_visitor)?;

    Ok(extract_server_visitor.server_spec)
}

#[derive(Debug)]
pub struct ExtractServerDataVisitor<'a> {
    pub server_spec: ChartSpec,
    supported_vars: HashMap<ScopedVariable, DataSupported>,
    task_scope: &'a mut TaskScope,
}

impl<'a> ExtractServerDataVisitor<'a> {
    pub fn new(
        supported_vars: HashMap<ScopedVariable, DataSupported>,
        task_scope: &'a mut TaskScope,
    ) -> Self {
        Self {
            server_spec: Default::default(),
            supported_vars,
            task_scope,
        }
    }
}

impl<'a> MutChartVisitor for ExtractServerDataVisitor<'a> {
    fn visit_data(&mut self, data: &mut DataSpec, scope: &[u32]) -> Result<()> {
        let data_var: ScopedVariable = (Variable::new_data(&data.name), Vec::from(scope));

        match self.supported_vars.get(&data_var) {
            Some(DataSupported::PartiallySupported) => {
                // Split transforms at first unsupported transform.
                // Note: There could be supported transforms in the client_tx after an unsupported
                // transform.
                let server_tx: Vec<_> = data
                    .transform
                    .iter()
                    .cloned()
                    .take_while(|tx| tx.supported())
                    .collect();

                let client_tx: Vec<_> = data
                    .transform
                    .iter()
                    .cloned()
                    .skip_while(|tx| tx.supported())
                    .collect();

                // Compute new name for server data
                let mut server_name = data.name.clone();
                server_name.insert_str(0, "_server_");

                // Clone data for use on server (with updated name)
                let mut server_data = data.clone();
                server_data.name = server_name.clone();
                server_data.transform = server_tx;

                let server_signals = server_data.output_signals();
                // Update server spec
                if scope.is_empty() {
                    self.server_spec.data.push(server_data)
                } else {
                    let server_group = self.server_spec.get_nested_group_mut(scope)?;
                    server_group.data.push(server_data);
                }

                // Update client data spec:
                //   - Same name
                //   - Add source of server
                //   - Update remaining transforms
                data.source = Some(server_name.clone());
                data.format = None;
                data.values = None;
                data.transform = client_tx;
                data.on = None;
                data.url = None;

                // Update scope
                //  - Add new data variable to task scope
                self.task_scope
                    .add_variable(&Variable::new_data(&server_name), scope)?;

                // - Handle signals generated by transforms that have been moved to the server spec
                for sig in &server_signals {
                    self.task_scope.remove_data_signal(sig, scope)?;
                    self.task_scope.add_data_signal(&server_name, sig, scope)?;
                }
            }
            _ => {
                // DataSupported::Supported

                // Add clone of full server data
                let server_data = data.clone();
                if scope.is_empty() {
                    self.server_spec.data.push(server_data)
                } else {
                    let server_group = self.server_spec.get_nested_group_mut(scope)?;
                    server_group.data.push(server_data);
                }

                // Clear everything except name from client spec
                data.format = None;
                data.source = None;
                data.values = None;
                data.transform = Vec::new();
                data.on = None;
                data.url = None;
            }
        }

        // if self.supported_vars.contains_key(&data_var) {
        //     // Add clone to server data
        //     let server_data = data.clone();
        //     if scope.is_empty() {
        //         self.server_spec.data.push(server_data)
        //     } else {
        //         let server_group = self.server_spec.get_nested_group_mut(scope)?;
        //         server_group.data.push(server_data);
        //     }
        //
        //     // Clear everything except name from client spec
        //     data.format = None;
        //     data.source = None;
        //     data.values = None;
        //     data.transform = Vec::new();
        //     data.on = None;
        //     data.url = None;
        // }
        Ok(())
    }

    fn visit_group_mark(&mut self, _mark: &mut MarkSpec, scope: &[u32]) -> Result<()> {
        // Initialize group mark in server spec
        let parent_scope = &scope[..scope.len() - 1];
        let new_group = MarkSpec {
            type_: "group".to_string(),
            name: None,
            from: None,
            encode: None,
            data: vec![],
            signals: vec![],
            marks: vec![],
            scales: vec![],
            extra: Default::default(),
        };
        if parent_scope.is_empty() {
            self.server_spec.marks.push(new_group);
        } else {
            let parent_group = self.server_spec.get_nested_group_mut(parent_scope)?;
            parent_group.marks.push(new_group);
        }

        Ok(())
    }
}
