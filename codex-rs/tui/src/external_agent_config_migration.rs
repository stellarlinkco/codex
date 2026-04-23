use crate::key_hint;
use crate::render::Insets;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::render::renderable::RenderableExt as _;
use crate::selection_list::selection_option_row;
use crate::tui::FrameRequester;
use crate::tui::Tui;
use crate::tui::TuiEvent;
use codex_core::external_agent_config::ExternalAgentConfigMigrationItem;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize as _;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;
use tokio_stream::StreamExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExternalAgentConfigMigrationOutcome {
    Import,
    Skip,
    SkipForever,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalAgentConfigMigrationSelection {
    Import,
    Skip,
    SkipForever,
}

impl ExternalAgentConfigMigrationSelection {
    fn next(self) -> Self {
        match self {
            Self::Import => Self::Skip,
            Self::Skip => Self::SkipForever,
            Self::SkipForever => Self::Import,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Import => Self::SkipForever,
            Self::Skip => Self::Import,
            Self::SkipForever => Self::Skip,
        }
    }

    fn outcome(self) -> ExternalAgentConfigMigrationOutcome {
        match self {
            Self::Import => ExternalAgentConfigMigrationOutcome::Import,
            Self::Skip => ExternalAgentConfigMigrationOutcome::Skip,
            Self::SkipForever => ExternalAgentConfigMigrationOutcome::SkipForever,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Import => "Import detected items",
            Self::Skip => "Not now",
            Self::SkipForever => "Don't ask again for these scopes",
        }
    }
}

pub(crate) async fn run_external_agent_config_migration_prompt(
    tui: &mut Tui,
    items: &[ExternalAgentConfigMigrationItem],
    error: Option<&str>,
) -> ExternalAgentConfigMigrationOutcome {
    let mut screen = ExternalAgentConfigMigrationScreen::new(
        tui.frame_requester(),
        items.to_vec(),
        error.map(ToOwned::to_owned),
    );
    let _ = tui.draw(u16::MAX, |frame| {
        frame.render_widget_ref(&screen, frame.area());
    });

    let events = tui.event_stream();
    tokio::pin!(events);

    while !screen.is_done() {
        if let Some(event) = events.next().await {
            match event {
                TuiEvent::Key(key_event) => screen.handle_key(key_event),
                TuiEvent::Paste(_) => {}
                TuiEvent::Draw => {
                    let _ = tui.draw(u16::MAX, |frame| {
                        frame.render_widget_ref(&screen, frame.area());
                    });
                }
            }
        } else {
            break;
        }
    }

    screen.outcome()
}

struct ExternalAgentConfigMigrationScreen {
    request_frame: FrameRequester,
    items: Vec<ExternalAgentConfigMigrationItem>,
    error: Option<String>,
    highlighted: ExternalAgentConfigMigrationSelection,
    done: bool,
    outcome: ExternalAgentConfigMigrationOutcome,
}

impl ExternalAgentConfigMigrationScreen {
    fn new(
        request_frame: FrameRequester,
        items: Vec<ExternalAgentConfigMigrationItem>,
        error: Option<String>,
    ) -> Self {
        Self {
            request_frame,
            items,
            error,
            highlighted: ExternalAgentConfigMigrationSelection::Import,
            done: false,
            outcome: ExternalAgentConfigMigrationOutcome::Skip,
        }
    }

    fn handle_key(&mut self, key_event: KeyEvent) {
        if key_event.kind == KeyEventKind::Release {
            return;
        }
        if key_event.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key_event.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            self.finish(ExternalAgentConfigMigrationOutcome::Exit);
            return;
        }

        match key_event.code {
            KeyCode::Up | KeyCode::Char('k') => self.set_highlight(self.highlighted.prev()),
            KeyCode::Down | KeyCode::Char('j') => self.set_highlight(self.highlighted.next()),
            KeyCode::Char('1') => self.finish(ExternalAgentConfigMigrationOutcome::Import),
            KeyCode::Char('2') | KeyCode::Esc => {
                self.finish(ExternalAgentConfigMigrationOutcome::Skip);
            }
            KeyCode::Char('3') => self.finish(ExternalAgentConfigMigrationOutcome::SkipForever),
            KeyCode::Enter => self.finish(self.highlighted.outcome()),
            _ => {}
        }
    }

    fn set_highlight(&mut self, highlighted: ExternalAgentConfigMigrationSelection) {
        if self.highlighted != highlighted {
            self.highlighted = highlighted;
            self.request_frame.schedule_frame();
        }
    }

    fn finish(&mut self, outcome: ExternalAgentConfigMigrationOutcome) {
        self.outcome = outcome;
        self.done = true;
        self.request_frame.schedule_frame();
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn outcome(&self) -> ExternalAgentConfigMigrationOutcome {
        self.outcome
    }
}

impl WidgetRef for &ExternalAgentConfigMigrationScreen {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let mut column = ColumnRenderable::new();
        column.push("");
        column.push(Line::from("External configuration detected").bold());
        column.push("");
        column.push(
            Paragraph::new(
                "Codex found configuration that can be imported from your external agent setup.",
            )
            .wrap(Wrap { trim: false })
            .inset(Insets::tlbr(0, 2, 0, 0)),
        );
        column.push("");

        for item in &self.items {
            column.push(
                Paragraph::new(format!("- {}", item.description))
                    .wrap(Wrap { trim: false })
                    .inset(Insets::tlbr(0, 2, 0, 0)),
            );
        }

        if let Some(error) = self.error.as_deref() {
            column.push("");
            column.push(Line::from(error).red().inset(Insets::tlbr(0, 2, 0, 0)));
        }

        column.push("");
        column.push(selection_option_row(
            0,
            ExternalAgentConfigMigrationSelection::Import
                .label()
                .to_string(),
            self.highlighted == ExternalAgentConfigMigrationSelection::Import,
        ));
        column.push(selection_option_row(
            1,
            ExternalAgentConfigMigrationSelection::Skip
                .label()
                .to_string(),
            self.highlighted == ExternalAgentConfigMigrationSelection::Skip,
        ));
        column.push(selection_option_row(
            2,
            ExternalAgentConfigMigrationSelection::SkipForever
                .label()
                .to_string(),
            self.highlighted == ExternalAgentConfigMigrationSelection::SkipForever,
        ));
        column.push("");
        column.push(
            Line::from(vec![
                "Use ".dim(),
                key_hint::plain(KeyCode::Up).into(),
                "/".dim(),
                key_hint::plain(KeyCode::Down).into(),
                " to move, press ".dim(),
                key_hint::plain(KeyCode::Enter).into(),
                " to confirm".dim(),
            ])
            .inset(Insets::tlbr(0, 2, 0, 0)),
        );
        column.render(area, buf);
    }
}
