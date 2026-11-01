use super::*;

impl ChatWidget {
    pub(crate) fn set_sidebar_visible(&mut self, visible: bool) {
        self.bottom_pane.set_status_line_enabled(!visible);
        if visible {
            let provider = self
                .sidebar_routes
                .routes
                .iter()
                .find(|route| {
                    route
                        .models
                        .iter()
                        .any(|model| model.slug == self.current_model())
                })
                .map(|route| route.provider.name.as_str())
                .unwrap_or(&self.config.model_provider_id);
            self.bottom_pane.set_status_line(Some(
                ratatui::text::Line::from(vec![
                    "Codex ".cyan(),
                    format!("· {} · {} · ", self.current_model(), provider).into(),
                    self.effective_reasoning_effort()
                        .map(|effort| effort.to_string())
                        .unwrap_or_default()
                        .fg(crate::terminal_palette::rgb_color((224, 175, 68))),
                ])
                .into(),
            ));
        }
    }
    pub(crate) fn sidebar_lines(&self) -> Vec<ratatui::text::Line<'static>> {
        use ratatui::text::Line;
        let mut lines = vec![
            Line::from(self.thread_name().unwrap_or_else(|| "New session".into())).bold(),
            Line::from(if self.bottom_pane.is_task_running() {
                "Working"
            } else {
                "Idle"
            }),
        ];
        if let Some(started) = self.turn_lifecycle.goal_status_active_turn_started_at {
            lines.push(Line::from(format!("{}s elapsed", started.elapsed().as_secs())).dim());
        }
        let goal_indicator = self.goal_status_indicator(Instant::now());
        if let Some(goal_line) =
            crate::bottom_pane::goal_status_indicator_line(goal_indicator.as_ref())
        {
            lines.push(Line::from(""));
            lines.push(goal_line);
        }
        lines.extend([Line::from(""), Line::from("Context").bold()]);
        if self.token_usage_pending {
            lines.push(Line::from("Updating context…").dim());
        } else if let Some(info) = &self.token_info {
            lines.push(Line::from(format!(
                "{} tokens",
                grouped_token_count(info.last_token_usage.total_tokens)
            )));
        } else {
            lines.push(Line::from("0 tokens").dim());
        }
        if let Some(used) = self.status_line_context_used_percent() {
            lines.push(Line::from(format!("{used}% used")));
        }
        if self.turn_lifecycle.agent_turn_running
            && let Some(active) = &self.sidebar_turn_model
            && active != self.current_model()
        {
            lines.push(Line::from(format!("Working: {active}")).dim());
            lines.push(Line::from("Selection applies next turn").cyan());
        }
        let servers = self
            .mcp_startup_status
            .as_ref()
            .unwrap_or(&self.sidebar_mcp_status);
        if !servers.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("MCP").bold());
            let mut servers = servers.iter().collect::<Vec<_>>();
            servers.sort_by_key(|(name, _)| *name);
            for (name, status) in servers {
                let label = match status {
                    McpStartupStatus::Starting => "starting",
                    McpStartupStatus::Ready => "connected",
                    McpStartupStatus::Failed { .. } => "failed",
                    McpStartupStatus::Cancelled => "cancelled",
                };
                let dot = if matches!(status, McpStartupStatus::Ready) {
                    "● ".green()
                } else {
                    "● ".red()
                };
                lines.push(Line::from(vec![
                    dot,
                    format!("{name} ").into(),
                    label.to_string().into(),
                ]));
            }
        }
        lines.extend([
            Line::from(""),
            Line::from("Workspace"),
            Line::from(self.config.cwd.display().to_string()),
            Line::from(""),
        ]);
        if let Some(branch) = &self.status_line_branch {
            lines.push(Line::from(branch.clone()).dim());
        }
        if let Some(count) = self.sidebar_dirty_files {
            lines.push(Line::from(format!("{count} changed files")).dim());
        }
        if let Some(connection) = &self.remote_connection
            && connection.version.trim_start_matches('v') != env!("CARGO_PKG_VERSION")
        {
            lines.push(Line::from(format!("Server version: {}", connection.version)).cyan());
        }
        lines.push(Line::from(""));
        let build = codex_build_info::BuildInfo::get();
        lines.push(Line::from(format!(
            "custom · {}",
            build.build_commit().chars().take(8).collect::<String>()
        )));
        lines.push(Line::from(vec![
            "● ".green(),
            format!("Codex {}", env!("CARGO_PKG_VERSION")).dim(),
        ]));
        lines
    }
}

fn grouped_token_count(tokens: i64) -> String {
    let digits = tokens.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > usize::from(tokens < 0) && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}
