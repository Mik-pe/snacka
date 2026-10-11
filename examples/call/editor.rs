// SPDX-License-Identifier: MIT
use super::profile::Profile;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub enum EditAction {
    None,
    Cancel,
    Connect,
    Disconnect,
}

pub struct Editor {
    pub draft: Profile,
    pub error: String,
    field: usize,
    cursor: usize,
}

const FIELDS: usize = 7;
impl Editor {
    pub fn new(profile: &Profile) -> Self {
        Self {
            draft: profile.clone(),
            error: String::new(),
            field: 2,
            cursor: profile.remote.chars().count(),
        }
    }
    fn text(&mut self) -> Option<&mut String> {
        match self.field {
            2 => Some(&mut self.draft.remote),
            3 => Some(&mut self.draft.bind),
            5 => Some(&mut self.draft.key),
            _ => None,
        }
    }
    fn focus(&mut self, field: usize) {
        self.field = field;
        self.cursor = self.text().map_or(0, |text| text.chars().count());
    }
    pub fn paste(&mut self, value: &str) {
        // Address and hex key fields are ASCII; discard multiline clipboard
        // control characters rather than executing unrelated shortcuts.
        let value: String = value
            .chars()
            .filter(|c| c.is_ascii_graphic())
            .take(256)
            .collect();
        let cursor = self.cursor;
        if let Some(text) = self.text()
            && text.len() + value.len() <= 256
        {
            let byte = text
                .char_indices()
                .nth(cursor)
                .map_or(text.len(), |(byte, _)| byte);
            text.insert_str(byte, &value);
            self.cursor += value.chars().count();
            self.error.clear();
        }
    }
    pub fn key(&mut self, key: KeyEvent) -> EditAction {
        match key.code {
            KeyCode::Esc => return EditAction::Cancel,
            KeyCode::Enter => {
                return if self.field == 6 {
                    EditAction::Disconnect
                } else {
                    EditAction::Connect
                };
            }
            KeyCode::Tab | KeyCode::Down => self.focus((self.field + 1) % FIELDS),
            KeyCode::BackTab | KeyCode::Up => self.focus((self.field + FIELDS - 1) % FIELDS),
            KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')
                if matches!(self.field, 0 | 1 | 4 | 6) =>
            {
                match self.field {
                    0 => self.draft.direct = !self.draft.direct,
                    1 => self.draft.family = self.draft.family.cycle(key.code == KeyCode::Left),
                    4 => self.draft.role = 3 - self.draft.role,
                    _ => {}
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text().map_or(0, |text| text.chars().count()),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => {
                let len = self.text().map_or(0, |text| text.chars().count());
                self.cursor = (self.cursor + 1).min(len);
            }
            KeyCode::Backspace => {
                let cursor = self.cursor;
                if let Some(text) = self.text()
                    && cursor > 0
                {
                    let byte = text
                        .char_indices()
                        .nth(cursor - 1)
                        .expect("cursor in field")
                        .0;
                    text.remove(byte);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                let cursor = self.cursor;
                if let Some(text) = self.text()
                    && cursor < text.chars().count()
                {
                    let byte = text.char_indices().nth(cursor).expect("cursor in field").0;
                    text.remove(byte);
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = self.text() {
                    text.clear();
                }
                self.cursor = 0;
            }
            KeyCode::Char(c)
                if c.is_ascii_graphic() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.paste(&c.to_string())
            }
            _ => {}
        }
        EditAction::None
    }

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        let width = area.width.min(82);
        let height = area.height.min(24);
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .title(" CONNECTION / CHOOSE YOUR LINK ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if inner.width < 36 || inner.height < 22 {
            frame.render_widget(Paragraph::new("Resize to at least 38 x 24 to edit the connection.\nEsc closes the panel; Ctrl-C quits.").wrap(Wrap { trim: false }), inner);
            return;
        }
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(14),
            Constraint::Min(2),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inner);
        frame.render_widget(Paragraph::new("Choose IPv4, IPv6 or a relay. Settings stay in memory.\nEnter replaces the current call; Esc keeps it."), rows[0]);
        let hint = if self.draft.direct && self.draft.family == super::profile::Family::Ipv6 {
            "[IPv6]:PORT"
        } else {
            "HOST:PORT or [IPv6]:PORT"
        };
        let values = [
            (
                "Mode",
                if self.draft.direct {
                    "‹ Direct / no server ›".into()
                } else {
                    "‹ UDP relay ›".into()
                },
            ),
            ("IP family", format!("‹ {} ›", self.draft.family.label())),
            ("Remote address", self.draft.remote.clone()),
            (
                "Local bind",
                if self.draft.bind.is_empty() {
                    "automatic".into()
                } else {
                    self.draft.bind.clone()
                },
            ),
            (
                "Your role",
                format!(
                    "{}  (other client uses {})",
                    self.draft.role,
                    3 - self.draft.role
                ),
            ),
            (
                "Shared room key",
                if self.draft.key.is_empty() {
                    "paste the same 64 hex digits on both clients".into()
                } else {
                    "*".repeat(self.draft.key.chars().count())
                },
            ),
            ("Hang up", "Disconnect and return to idle".into()),
        ];
        for (index, ((label, value), row)) in values
            .iter()
            .zip(
                Layout::vertical([Constraint::Length(2); FIELDS])
                    .split(rows[1])
                    .iter(),
            )
            .enumerate()
        {
            let style = Style::default().fg(if self.field == index {
                Color::Cyan
            } else {
                Color::Gray
            });
            let prefix = if self.field == index { "›" } else { " " };
            // Show the cursor's neighborhood even for long IPv6/hostname fields.
            let available = row.width.saturating_sub(3) as usize;
            let offset = if self.field == index && matches!(index, 2 | 3 | 5) {
                self.cursor.saturating_sub(available.saturating_sub(1))
            } else {
                0
            };
            let visible: String = value.chars().skip(offset).take(available).collect();
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        format!("{prefix} {label}"),
                        style.add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(format!("  {visible}"), style)),
                ]),
                *row,
            );
            if self.field == index && matches!(index, 2 | 3 | 5) {
                frame.set_cursor_position((row.x + 2 + (self.cursor - offset) as u16, row.y + 1));
            }
        }
        frame.render_widget(
            Paragraph::new(&*self.error)
                .style(Style::default().fg(Color::Yellow))
                .wrap(Wrap { trim: true }),
            rows[2],
        );
        frame.render_widget(Paragraph::new(if self.draft.direct {
            format!("Remote: {hint}. IPv4 internet needs port forwarding;\nIPv6 needs reachable addresses and inbound UDP allowed.")
        } else { "Both clients connect outwards to the same relay.\nUse the same key on the relay; it can read the audio.".into() }).style(Style::default().fg(Color::DarkGray)), rows[3]);
        frame.render_widget(Paragraph::new("Tab / ↑↓ field   ←→ choice   Ctrl-U clear field\nEnter connect / hang up   Esc cancel   Ctrl-C quit").style(Style::default().fg(Color::Yellow)), rows[4]);
    }
}

#[cfg(test)]
mod tests {
    use super::super::profile::Family;
    use super::*;
    fn editor() -> Editor {
        Editor::new(&Profile {
            direct: true,
            role: 1,
            family: Family::Auto,
            remote: String::new(),
            bind: String::new(),
            key: "ab".repeat(32),
        })
    }
    fn key(editor: &mut Editor, code: KeyCode) -> EditAction {
        editor.key(KeyEvent::new(code, KeyModifiers::NONE))
    }
    #[test]
    fn edits_pasted_addresses_choices_and_secrets_without_triggering_call_shortcuts() {
        let mut editor = editor();
        editor.paste("127.0.0.1:5000\n");
        key(&mut editor, KeyCode::Char('m'));
        key(&mut editor, KeyCode::Backspace);
        assert_eq!(editor.draft.remote, "127.0.0.1:5000");
        key(&mut editor, KeyCode::Home);
        key(&mut editor, KeyCode::Delete);
        assert_eq!(editor.draft.remote, "27.0.0.1:5000");
        editor.focus(1);
        key(&mut editor, KeyCode::Right);
        assert!(editor.draft.family == Family::Ipv4);
        editor.focus(0);
        key(&mut editor, KeyCode::Right);
        assert!(!editor.draft.direct);
        editor.focus(4);
        key(&mut editor, KeyCode::Left);
        assert_eq!(editor.draft.role, 2);
        assert!(matches!(
            key(&mut editor, KeyCode::Enter),
            EditAction::Connect
        ));
        editor.focus(6);
        assert!(matches!(
            key(&mut editor, KeyCode::Enter),
            EditAction::Disconnect
        ));
    }
    #[test]
    fn popup_masks_the_key_and_is_safe_in_small_terminals() {
        for (width, height) in [(100, 34), (64, 25), (40, 24), (40, 22), (20, 8)] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
            let editor = editor();
            terminal.draw(|frame| editor.draw(frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(!text.contains(&editor.draft.key));
            if width >= 40 && height >= 24 {
                assert!(text.contains("********"), "{width}x{height}: {text}");
            }
        }
    }
}
