use crate::history_cell::PlainHistoryCell;
use crate::render::line_utils::prefix_lines;
use crate::text_formatting::truncate_text;
use codex_core::protocol::AgentStatus;
use codex_core::protocol::CollabAgentInteractionEndEvent;
use codex_core::protocol::CollabAgentSpawnEndEvent;
use codex_core::protocol::CollabCloseEndEvent;
use codex_core::protocol::CollabResumeBeginEvent;
use codex_core::protocol::CollabResumeEndEvent;
use codex_core::protocol::CollabWaitingBeginEvent;
use codex_core::protocol::CollabWaitingEndEvent;
use codex_protocol::ThreadId;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use std::collections::HashMap;

const COLLAB_PROMPT_PREVIEW_GRAPHEMES: usize = 160;
const COLLAB_AGENT_ERROR_PREVIEW_GRAPHEMES: usize = 160;
const COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES: usize = 240;
const TEAM_SPAWN_CALL_PREFIX: &str = "team/spawn:";
const TEAM_WAIT_CALL_PREFIX: &str = "team/wait:";
const TEAM_CLOSE_CALL_PREFIX: &str = "team/close:";

pub(crate) fn spawn_end(ev: CollabAgentSpawnEndEvent) -> PlainHistoryCell {
    let CollabAgentSpawnEndEvent {
        call_id,
        sender_thread_id: _,
        new_thread_id,
        prompt,
        status,
    } = ev;
    let new_agent = new_thread_id
        .map(|id| Span::from(short_thread_id(&id)))
        .unwrap_or_else(|| Span::from("not created").dim());
    let mut details = vec![
        detail_line("call", call_id),
        detail_line("agent", new_agent),
        status_line(&status),
    ];
    if let Some(line) = prompt_line(&prompt) {
        details.push(line);
    }
    collab_event("Agent spawned", details)
}

pub(crate) fn interaction_end(ev: CollabAgentInteractionEndEvent) -> PlainHistoryCell {
    let CollabAgentInteractionEndEvent {
        call_id,
        sender_thread_id: _,
        receiver_thread_id,
        prompt,
        status,
    } = ev;
    let mut details = vec![
        detail_line("call", call_id),
        detail_line("receiver", short_thread_id(&receiver_thread_id)),
        status_line(&status),
    ];
    if let Some(line) = prompt_line(&prompt) {
        details.push(line);
    }
    collab_event("Input sent", details)
}

pub(crate) fn waiting_begin(ev: CollabWaitingBeginEvent) -> PlainHistoryCell {
    let CollabWaitingBeginEvent {
        call_id,
        sender_thread_id: _,
        receiver_thread_ids,
        receiver_names,
    } = ev;
    let title = if call_id.starts_with(TEAM_SPAWN_CALL_PREFIX) {
        "Spawning team"
    } else if call_id.starts_with(TEAM_WAIT_CALL_PREFIX) {
        "Waiting for team"
    } else if call_id.starts_with(TEAM_CLOSE_CALL_PREFIX) {
        "Closing team"
    } else {
        "Waiting for agents"
    };
    let details = vec![
        detail_line("call", call_id),
        detail_line(
            "receivers",
            format_thread_ids(&receiver_thread_ids, &receiver_names),
        ),
    ];
    collab_event(title, details)
}

pub(crate) fn waiting_end(ev: CollabWaitingEndEvent) -> PlainHistoryCell {
    let CollabWaitingEndEvent {
        call_id,
        sender_thread_id: _,
        statuses,
        receiver_names,
    } = ev;
    let title = if call_id.starts_with(TEAM_SPAWN_CALL_PREFIX) {
        "Team spawned"
    } else if call_id.starts_with(TEAM_WAIT_CALL_PREFIX) {
        "Team wait complete"
    } else if call_id.starts_with(TEAM_CLOSE_CALL_PREFIX) {
        "Team close complete"
    } else {
        "Wait complete"
    };
    let mut details = vec![detail_line("call", call_id)];
    details.extend(wait_complete_lines(&statuses, &receiver_names));
    collab_event(title, details)
}

pub(crate) fn close_end(ev: CollabCloseEndEvent) -> PlainHistoryCell {
    let CollabCloseEndEvent {
        call_id,
        sender_thread_id: _,
        receiver_thread_id,
        status,
    } = ev;
    let details = vec![
        detail_line("call", call_id),
        detail_line("receiver", short_thread_id(&receiver_thread_id)),
        status_line(&status),
    ];
    collab_event("Agent closed", details)
}

pub(crate) fn resume_begin(ev: CollabResumeBeginEvent) -> PlainHistoryCell {
    let CollabResumeBeginEvent {
        call_id,
        sender_thread_id: _,
        receiver_thread_id,
    } = ev;
    let details = vec![
        detail_line("call", call_id),
        detail_line("receiver", short_thread_id(&receiver_thread_id)),
    ];
    collab_event("Resuming agent", details)
}

pub(crate) fn resume_end(ev: CollabResumeEndEvent) -> PlainHistoryCell {
    let CollabResumeEndEvent {
        call_id,
        sender_thread_id: _,
        receiver_thread_id,
        status,
    } = ev;
    let details = vec![
        detail_line("call", call_id),
        detail_line("receiver", short_thread_id(&receiver_thread_id)),
        status_line(&status),
    ];
    collab_event("Agent resumed", details)
}

fn collab_event(title: impl Into<String>, details: Vec<Line<'static>>) -> PlainHistoryCell {
    let title = title.into();
    let mut lines: Vec<Line<'static>> =
        vec![vec![Span::from("• ").dim(), Span::from(title).bold()].into()];
    if !details.is_empty() {
        lines.extend(prefix_lines(details, "  └ ".dim(), "    ".into()));
    }
    PlainHistoryCell::new(lines)
}

fn detail_line(label: &str, value: impl Into<Span<'static>>) -> Line<'static> {
    vec![Span::from(format!("{label}: ")).dim(), value.into()].into()
}

fn status_line(status: &AgentStatus) -> Line<'static> {
    detail_line("status", status_span(status))
}

fn status_span(status: &AgentStatus) -> Span<'static> {
    match status {
        AgentStatus::PendingInit => Span::from("pending init").dim(),
        AgentStatus::Running => Span::from("▶ running").cyan().bold(),
        AgentStatus::Completed(_) => Span::from("completed").green(),
        AgentStatus::Errored(_) => Span::from("errored").red(),
        AgentStatus::Shutdown => Span::from("shutdown").dim(),
        AgentStatus::NotFound => Span::from("not found").red(),
    }
}

fn short_thread_id(id: &ThreadId) -> String {
    id.to_string().chars().take(8).collect()
}

fn prompt_line(prompt: &str) -> Option<Line<'static>> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(detail_line(
            "prompt",
            Span::from(truncate_text(trimmed, COLLAB_PROMPT_PREVIEW_GRAPHEMES)).dim(),
        ))
    }
}

fn receiver_label(id: &ThreadId, receiver_names: &HashMap<ThreadId, String>) -> String {
    let short_id = short_thread_id(id);
    match receiver_names.get(id) {
        Some(name) if !name.trim().is_empty() => format!("{name} ({short_id})"),
        _ => short_id,
    }
}

fn format_thread_ids(
    ids: &[ThreadId],
    receiver_names: &HashMap<ThreadId, String>,
) -> Span<'static> {
    if ids.is_empty() {
        return Span::from("none").dim();
    }
    let joined = ids
        .iter()
        .map(|id| receiver_label(id, receiver_names))
        .collect::<Vec<_>>()
        .join(", ");
    Span::from(joined)
}

fn wait_complete_lines(
    statuses: &HashMap<ThreadId, AgentStatus>,
    receiver_names: &HashMap<ThreadId, String>,
) -> Vec<Line<'static>> {
    if statuses.is_empty() {
        return vec![detail_line("agents", Span::from("none").dim())];
    }

    let mut pending_init = 0usize;
    let mut running = 0usize;
    let mut completed = 0usize;
    let mut errored = 0usize;
    let mut shutdown = 0usize;
    let mut not_found = 0usize;
    for status in statuses.values() {
        match status {
            AgentStatus::PendingInit => pending_init += 1,
            AgentStatus::Running => running += 1,
            AgentStatus::Completed(_) => completed += 1,
            AgentStatus::Errored(_) => errored += 1,
            AgentStatus::Shutdown => shutdown += 1,
            AgentStatus::NotFound => not_found += 1,
        }
    }

    let mut summary = vec![Span::from(format!("{} total", statuses.len())).dim()];
    push_status_count(
        &mut summary,
        pending_init,
        "pending init",
        ratatui::prelude::Stylize::dim,
    );
    push_status_count(&mut summary, running, "running", |span| span.cyan().bold());
    push_status_count(
        &mut summary,
        completed,
        "completed",
        ratatui::prelude::Stylize::green,
    );
    push_status_count(
        &mut summary,
        errored,
        "errored",
        ratatui::prelude::Stylize::red,
    );
    push_status_count(
        &mut summary,
        shutdown,
        "shutdown",
        ratatui::prelude::Stylize::dim,
    );
    push_status_count(
        &mut summary,
        not_found,
        "not found",
        ratatui::prelude::Stylize::red,
    );

    let mut entries: Vec<(String, String, &AgentStatus)> = statuses
        .iter()
        .map(|(thread_id, status)| {
            let thread_id_str = thread_id.to_string();
            (
                receiver_label(thread_id, receiver_names),
                thread_id_str,
                status,
            )
        })
        .collect();
    entries.sort_by(|(left_name, left_id, _), (right_name, right_id, _)| {
        left_name
            .cmp(right_name)
            .then_with(|| left_id.cmp(right_id))
    });

    let mut lines = Vec::with_capacity(entries.len() + 1);
    lines.push(detail_line_spans("agents", summary));
    lines.extend(entries.into_iter().map(|(receiver, _, status)| {
        let mut spans = vec![
            Span::from(receiver).dim(),
            Span::from(" ").dim(),
            status_span(status),
        ];
        match status {
            AgentStatus::Completed(Some(message)) => {
                let message_preview = truncate_text(
                    &message.split_whitespace().collect::<Vec<_>>().join(" "),
                    COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES,
                );
                spans.push(Span::from(": ").dim());
                spans.push(Span::from(message_preview));
            }
            AgentStatus::Errored(error) => {
                let error_preview = truncate_text(
                    &error.split_whitespace().collect::<Vec<_>>().join(" "),
                    COLLAB_AGENT_ERROR_PREVIEW_GRAPHEMES,
                );
                spans.push(Span::from(": ").dim());
                spans.push(Span::from(error_preview).dim());
            }
            _ => {}
        }
        spans.into()
    }));
    lines
}

fn push_status_count(
    spans: &mut Vec<Span<'static>>,
    count: usize,
    label: &'static str,
    style: impl FnOnce(Span<'static>) -> Span<'static>,
) {
    if count == 0 {
        return;
    }

    spans.push(Span::from(" · ").dim());
    spans.push(style(Span::from(format!("{count} {label}"))));
}

fn detail_line_spans(label: &str, mut value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = Vec::with_capacity(value.len() + 1);
    spans.push(Span::from(format!("{label}: ")).dim());
    spans.append(&mut value);
    spans.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_cell::HistoryCell;
    use insta::assert_snapshot;

    fn render_cell(cell: PlainHistoryCell) -> String {
        cell.display_lines(200)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn waiting_begin_team_close_renders_receiver_name() {
        let sender_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("valid id");
        let receiver_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000002").expect("valid id");
        let receiver_names = HashMap::from([(receiver_thread_id, "explorer_agent".to_string())]);
        let rendered = render_cell(waiting_begin(CollabWaitingBeginEvent {
            sender_thread_id,
            receiver_thread_ids: vec![receiver_thread_id],
            receiver_names,
            call_id: format!("{TEAM_CLOSE_CALL_PREFIX}call-1"),
        }));
        assert_snapshot!(
            rendered,
            @r"
        • Closing team
          └ call: team/close:call-1
            receivers: explorer_agent (00000000)
        "
        );
    }

    #[test]
    fn waiting_end_team_close_renders_shutdown_status() {
        let sender_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("valid id");
        let receiver_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000002").expect("valid id");
        let statuses = HashMap::from([(receiver_thread_id, AgentStatus::Shutdown)]);
        let receiver_names = HashMap::from([(receiver_thread_id, "explorer_agent".to_string())]);
        let rendered = render_cell(waiting_end(CollabWaitingEndEvent {
            sender_thread_id,
            call_id: format!("{TEAM_CLOSE_CALL_PREFIX}call-2"),
            statuses,
            receiver_names,
        }));
        assert_snapshot!(
            rendered,
            @r"
        • Team close complete
          └ call: team/close:call-2
            agents: 1 total · 1 shutdown
            explorer_agent (00000000) shutdown
        "
        );
    }

    #[test]
    fn waiting_end_team_wait_renders_running_status_with_typed_receiver_name() {
        let sender_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000001").expect("valid id");
        let receiver_thread_id =
            ThreadId::from_string("00000000-0000-0000-0000-000000000002").expect("valid id");
        let statuses = HashMap::from([(receiver_thread_id, AgentStatus::Running)]);
        let receiver_names =
            HashMap::from([(receiver_thread_id, "reviewer [code-review]".to_string())]);
        let rendered = render_cell(waiting_end(CollabWaitingEndEvent {
            sender_thread_id,
            call_id: format!("{TEAM_WAIT_CALL_PREFIX}call-3"),
            statuses,
            receiver_names,
        }));
        assert_snapshot!(
            rendered,
            @r"
        • Team wait complete
          └ call: team/wait:call-3
            agents: 1 total · 1 running
            reviewer [code-review] (00000000) ▶ running
        "
        );
    }
}
