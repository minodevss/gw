use crossterm::event::KeyEvent;
use proto::features::{Feature, ThinkingKind};
use proto::ir::Effort;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, TableState};

use super::Request;
use super::keymap::Command;
use super::widgets::{
    Ctx, FilterAction, FilterBox, Selection, Theme, field_line, truncate, truncate_list,
};
use crate::catalog::view::ModelView;
use crate::catalog::{Capabilities, Support, feature_name};

const FEATURES: [Feature; 9] = [
    Feature::Effort(Effort::Xhigh),
    Feature::Effort(Effort::Max),
    Feature::Thinking(ThinkingKind::Adaptive),
    Feature::Fast,
    Feature::ImageInput,
    Feature::DocumentInput,
    Feature::StructuredOutput,
    Feature::Temperature,
    Feature::MaxOutputTokens,
];
const LABEL_WIDTH: usize = 20;
const ACCOUNT_WIDTH: usize = 18;
const OVERLAY_WIDTH: usize = 120;

type Matrix = (String, Vec<(String, Capabilities)>);

#[derive(Debug, Default)]
pub(crate) struct ModelsView {
    models: Vec<ModelView>,
    capabilities: Option<Matrix>,
    selection: Selection<String>,
    filter: FilterBox,
    table: TableState,
}

fn feature_label(feature: Feature) -> String {
    match feature {
        Feature::Effort(effort) => format!("effort {}", effort.as_str()),
        Feature::Thinking(kind) => format!("thinking {}", kind.as_str()),
        Feature::Fast
        | Feature::ImageInput
        | Feature::DocumentInput
        | Feature::StructuredOutput
        | Feature::StrictTools
        | Feature::CustomTools
        | Feature::MidConversationSystem
        | Feature::Temperature
        | Feature::TopP
        | Feature::MaxOutputTokens
        | Feature::Stop
        | Feature::User => feature_name(feature).to_owned(),
    }
}

fn mark(support: Support) -> &'static str {
    match support {
        Support::Yes => "✓",
        Support::No => "✗",
        Support::Unknown => "?",
    }
}

fn effort_text(efforts: Option<&[String]>) -> String {
    match efforts {
        None => "?".to_owned(),
        Some([]) => "—".to_owned(),
        Some([only]) => only.clone(),
        Some([first, .., last]) => format!("{first}…{last}"),
    }
}

fn yes_no(listed: bool) -> &'static str {
    if listed { "✓" } else { "✗" }
}

pub(crate) fn detail_lines(
    model: &ModelView,
    matrix: Option<&Matrix>,
    width: usize,
    theme: Theme,
) -> Vec<Line<'static>> {
    let id = &model.id;
    let mut lines = vec![
        field_line("names", format!("{id} · {id}:sub · {id}:api"), theme),
        field_line(
            "listed",
            format!(
                "/codex/models ✓ · /v1/models ✓ · Claude Code picker {} (only ids containing claude)",
                yes_no(id.contains("claude"))
            ),
            theme,
        ),
    ];
    let Some((_, accounts)) = matrix.filter(|(loaded, _)| loaded == id) else {
        lines.push(field_line("support", "loading…", theme));
        return lines;
    };
    let fit = (width.saturating_sub(LABEL_WIDTH) / ACCOUNT_WIDTH).max(1);
    let shown = accounts.iter().take(fit);
    let mut header = vec![Span::raw(" ".repeat(LABEL_WIDTH))];
    header.extend(shown.clone().map(|(name, _)| {
        Span::styled(
            format!("{:<ACCOUNT_WIDTH$}", truncate(name, ACCOUNT_WIDTH - 2)),
            theme.dim(),
        )
    }));
    if accounts.len() > fit {
        header.push(Span::styled(
            format!("+{}", accounts.len() - fit),
            theme.dim(),
        ));
    }
    lines.push(Line::default());
    lines.push(Line::from(header));
    for feature in FEATURES {
        let mut row = vec![Span::styled(
            format!("{:<LABEL_WIDTH$}", feature_label(feature)),
            theme.dim(),
        )];
        for (_, caps) in shown.clone() {
            let support = caps.support(feature);
            let style = match support {
                Support::Yes => theme.ok(),
                Support::No => theme.bad(),
                Support::Unknown => theme.dim(),
            };
            let star = if caps.overridden.iter().any(|n| n == feature_name(feature)) {
                "*"
            } else {
                ""
            };
            row.push(Span::styled(
                format!("{:<ACCOUNT_WIDTH$}", format!("{}{star}", mark(support))),
                style,
            ));
        }
        lines.push(Line::from(row));
    }
    lines
}

impl ModelsView {
    pub(crate) fn update(&mut self, models: Vec<ModelView>, capabilities: Option<Matrix>) {
        self.models = models;
        self.capabilities = capabilities;
        let ids = self.ids();
        self.selection.sync(&ids);
    }

    fn visible(&self) -> Vec<&ModelView> {
        let needle = self.filter.applied().to_lowercase();
        self.models
            .iter()
            .filter(|model| {
                needle.is_empty()
                    || model.id.to_lowercase().contains(&needle)
                    || model
                        .accounts
                        .iter()
                        .any(|account| account.to_lowercase().contains(&needle))
            })
            .collect()
    }

    fn ids(&self) -> Vec<String> {
        self.visible().iter().map(|m| m.id.clone()).collect()
    }

    fn selected(&self) -> Option<&ModelView> {
        let ids = self.ids();
        let id = ids.get(self.selection.index(&ids))?;
        self.models.iter().find(|m| &m.id == id)
    }

    pub(crate) fn selected_id(&self) -> Option<String> {
        self.selected().map(|m| m.id.clone())
    }

    pub(crate) fn is_editing(&self) -> bool {
        self.filter.is_editing()
    }

    pub(crate) fn paste(&mut self, pasted: &str) {
        self.filter.paste(pasted);
    }

    pub(crate) fn edit_filter(&mut self, key: &KeyEvent) {
        match self.filter.handle(key) {
            FilterAction::Submit(text) => self.filter.accept(text),
            FilterAction::Pending | FilterAction::Cleared => {}
        }
        let ids = self.ids();
        self.selection.sync(&ids);
    }

    pub(crate) fn command(&mut self, command: Command, theme: Theme) -> Option<Request> {
        let ids = self.ids();
        match command {
            Command::Down | Command::Up | Command::Top | Command::Bottom => {
                self.selection.apply(&ids, command);
                None
            }
            Command::Filter => {
                self.filter.start();
                None
            }
            Command::Back => {
                self.filter.clear();
                None
            }
            Command::Refresh => Some(Request::RefreshAll),
            Command::Copy => self.selected_id().map(Request::Copy),
            Command::Enter => {
                let model = self.selected()?;
                Some(Request::Lines {
                    title: model.id.clone(),
                    lines: detail_lines(model, self.capabilities.as_ref(), OVERLAY_WIDTH, theme),
                })
            }
            Command::Goto(_)
            | Command::New
            | Command::Login
            | Command::Remove
            | Command::Help
            | Command::Quit => None,
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        let detail = self.selected().map(|model| {
            detail_lines(
                model,
                self.capabilities.as_ref(),
                usize::from(area.width),
                ctx.theme,
            )
        });
        let wanted = detail.as_ref().map_or(1, |lines| lines.len() + 1);
        let detail_height = u16::try_from(wanted)
            .unwrap_or(u16::MAX)
            .min(area.height * 3 / 5);
        let filter_line = self.filter.line(ctx.theme);
        let [list, bar_area, detail_area] = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(u16::from(filter_line.is_some())),
            Constraint::Length(detail_height),
        ])
        .areas(area);
        if let Some(line) = filter_line {
            frame.render_widget(Paragraph::new(line), bar_area);
        }
        self.render_table(frame, list, ctx);
        let title = self
            .selected()
            .map_or_else(String::new, |m| format!(" {} ", m.id));
        frame.render_widget(
            Paragraph::new(detail.unwrap_or_default())
                .block(Block::new().borders(Borders::TOP).title(title)),
            detail_area,
        );
    }

    fn render_table(&mut self, frame: &mut Frame<'_>, area: Rect, ctx: &Ctx) {
        const FIXED: u16 = 24 + 7 + 10 + 5 + 10 + 1 + 6;
        let accounts_width = usize::from(area.width.saturating_sub(FIXED)).max(4);
        let models = self.visible();
        let rows: Vec<Row<'_>> = models
            .iter()
            .map(|model| {
                Row::new(vec![
                    truncate(&model.id, 24),
                    truncate_list(&model.accounts, accounts_width),
                    model
                        .context
                        .map_or_else(|| "?".to_owned(), |c| format!("{}k", c / 1000)),
                    effort_text(model.efforts.as_deref()),
                    if model.fast { "fast" } else { "—" }.to_owned(),
                    if model.image { "text image" } else { "text" }.to_owned(),
                ])
            })
            .collect();
        let ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
        self.table.select(if ids.is_empty() {
            None
        } else {
            Some(self.selection.index(&ids))
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(24),
                Constraint::Fill(1),
                Constraint::Length(7),
                Constraint::Length(10),
                Constraint::Length(5),
                Constraint::Length(10),
            ],
        )
        .header(
            Row::new(["MODEL", "ACCOUNTS", "CTX", "EFFORT", "SPEED", "INPUT"])
                .style(ctx.theme.dim().add_modifier(Modifier::BOLD)),
        )
        .column_spacing(1)
        .highlight_symbol("▶")
        .row_highlight_style(Style::new().add_modifier(Modifier::BOLD));
        frame.render_stateful_widget(table, area, &mut self.table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::widgets::testing;

    fn model(id: &str, accounts: &[&str]) -> ModelView {
        ModelView {
            id: id.to_owned(),
            accounts: accounts.iter().map(|a| (*a).to_owned()).collect(),
            context: Some(200_000),
            efforts: Some(vec!["low".to_owned(), "max".to_owned()]),
            fast: true,
            image: true,
            ..ModelView::default()
        }
    }

    fn fixture() -> ModelsView {
        let caps = Capabilities {
            efforts: Some(vec!["low".to_owned(), "xhigh".to_owned()]),
            fast: Some(true),
            image: Some(false),
            unsupported: vec!["temperature".to_owned()],
            overridden: vec!["fast".to_owned()],
            ..Capabilities::default()
        };
        let mut view = ModelsView::default();
        view.update(
            vec![
                model(
                    "claude-opus-5-5",
                    &[
                        "anthropic-oauth-1",
                        "anthropic-oauth-2",
                        "anthropic-oauth-3",
                    ],
                ),
                model("gpt-6.1-sol", &["openai-codex-1", "openai-siwc-1"]),
            ],
            Some((
                "claude-opus-5-5".to_owned(),
                vec![
                    ("anthropic-oauth-1".to_owned(), caps.clone()),
                    ("anthropic-oauth-2".to_owned(), Capabilities::default()),
                ],
            )),
        );
        view
    }

    fn ctx() -> Ctx {
        Ctx {
            now: 0,
            theme: Theme::new(false),
        }
    }

    #[test]
    fn filter_matches_model_id_and_account_names() {
        let mut view = fixture();
        view.filter.accept("siwc".to_owned());
        assert_eq!(view.ids(), vec!["gpt-6.1-sol".to_owned()]);
        view.filter.accept("OPUS".to_owned());
        assert_eq!(view.ids(), vec!["claude-opus-5-5".to_owned()]);
    }

    #[test]
    fn claude_picker_lists_only_claude_ids() {
        let theme = Theme::new(false);
        let text = |m: &ModelView| {
            detail_lines(m, None, 100, theme)
                .iter()
                .map(ToString::to_string)
                .collect::<String>()
        };
        assert!(text(&model("claude-x", &["a"])).contains("picker ✓"));
        assert!(text(&model("gpt-x", &["a"])).contains("picker ✗"));
    }

    #[test]
    fn effort_range_uses_first_and_last() {
        assert_eq!(
            effort_text(Some(&["low".to_owned(), "max".to_owned()])),
            "low…max"
        );
        assert_eq!(effort_text(None), "?");
    }

    #[test]
    fn renders_models_screen() {
        let mut view = fixture();
        let text = testing::render(100, 22, |frame| view.render(frame, frame.area(), &ctx()));
        insta::assert_snapshot!("models", text);
    }
}
