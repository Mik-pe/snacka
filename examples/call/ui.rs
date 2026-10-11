// SPDX-License-Identifier: MIT
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Sparkline, Wrap},
};
use std::{
    collections::VecDeque,
    io::{self, IsTerminal},
};

const CYAN: Color = Color::Rgb(66, 230, 204);
const GOLD: Color = Color::Rgb(255, 199, 87);
const DIM: Color = Color::Rgb(130, 148, 164);

pub struct Dashboard {
    pub mode: String,
    pub local: String,
    pub remote: String,
    pub input: String,
    pub output: String,
    pub connected: bool,
    pub registered: bool,
    pub seconds: f64,
    pub bitrate: u32,
    pub gain: f32,
    pub muted: bool,
    pub aec: bool,
    pub ns: bool,
    pub agc: bool,
    pub buffer_ms: u64,
    pub tx: usize,
    pub rx: usize,
    pub decoded: usize,
    pub concealed: usize,
    pub rejected: usize,
    pub drops: usize,
    pub gaps: usize,
    pub queue_ms: usize,
    pub local_level: f32,
    pub remote_level: f32,
    history: VecDeque<u64>,
    logs: VecDeque<String>,
}

impl Dashboard {
    pub fn new(mode: &str, local: String, remote: String) -> Self {
        Self {
            mode: mode.into(),
            local,
            remote,
            input: String::new(),
            output: String::new(),
            connected: false,
            registered: false,
            seconds: 0.0,
            bitrate: 24000,
            gain: 0.5,
            muted: false,
            aec: true,
            ns: true,
            agc: true,
            buffer_ms: 60,
            tx: 0,
            rx: 0,
            decoded: 0,
            concealed: 0,
            rejected: 0,
            drops: 0,
            gaps: 0,
            queue_ms: 0,
            local_level: 0.0,
            remote_level: 0.0,
            history: VecDeque::from(vec![0; 80]),
            logs: VecDeque::new(),
        }
    }

    pub fn log(&mut self, message: impl Into<String>) {
        if self.logs.len() == 5 {
            self.logs.pop_front();
        }
        self.logs.push_back(message.into());
    }

    pub fn sample(&mut self) {
        self.history.pop_front();
        self.history
            .push_back((meter(self.remote_level) * 100.0) as u64);
    }

    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 64 || area.height < 25 {
            frame.render_widget(Paragraph::new(format!(
                "SNACKA / {}\n{}  {:02}:{:02}\nTX {}  RX {}  PLC {}\nMute {}  AEC {}  NS {}  AGC {}\n\nResize to 64 x 25 for the full call panel.\nM mute / arrows volume + bitrate / Q quit",
                self.mode, self.status(), self.seconds as usize / 60, self.seconds as usize % 60,
                self.tx, self.rx, self.concealed, self.muted, self.aec, self.ns, self.agc
            )).style(Style::default().fg(CYAN)).wrap(Wrap { trim: false }), area);
            return;
        }
        let rows = Layout::vertical([
            Constraint::Length(4),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Length(4),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .margin(1)
        .split(area);
        let header = Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    " S N A C K A ",
                    Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
                ),
                Span::styled("  /  VOICE LINK", Style::default().fg(DIM)),
            ]),
            Line::from(vec![
                Span::styled(
                    format!(" {} ", self.status()),
                    Style::default().fg(if self.connected { CYAN } else { GOLD }),
                ),
                Span::raw(format!(
                    "   {}   •   XChaCha20-Poly1305   •   {:02}:{:02}",
                    self.mode,
                    self.seconds as usize / 60,
                    self.seconds as usize % 60
                )),
            ]),
        ])
        .block(panel(" LIVE SESSION "));
        frame.render_widget(header, rows[0]);

        let peers = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(rows[1]);
        self.peer_card(frame, peers[0], true);
        self.peer_card(frame, peers[1], false);

        let stats = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
            .split(rows[2]);
        let loss = if self.decoded + self.concealed == 0 {
            0.0
        } else {
            self.concealed as f64 * 100.0 / (self.decoded + self.concealed) as f64
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    " TX {:>7}    RX {:>7}    {} kb/s",
                    self.tx,
                    self.rx,
                    self.bitrate / 1000
                )),
                Line::from(format!(
                    " PLC {:>6} ({loss:.1}%)  reject/drop {}",
                    self.concealed, self.rejected
                )),
                Line::from(format!(
                    " Jitter {} ms  queue {} ms  gaps {}  drops {}",
                    self.buffer_ms, self.queue_ms, self.gaps, self.drops
                )),
            ])
            .block(panel(" NETWORK / AUDIO HEALTH ")),
            stats[0],
        );
        let history_width = panel(" REMOTE VOICE / LIVE ").inner(stats[1]).width as usize;
        let data: Vec<_> = self
            .history
            .iter()
            .skip(self.history.len().saturating_sub(history_width))
            .copied()
            .collect();
        frame.render_widget(
            Sparkline::default()
                .data(&data)
                .max(100)
                .style(Style::default().fg(CYAN))
                .block(panel(" REMOTE VOICE / LIVE ")),
            stats[1],
        );

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    toggle("[A] ECHO CANCEL", self.aec),
                    Span::raw("    "),
                    toggle("[N] NOISE FILTER", self.ns),
                    Span::raw("    "),
                    toggle("[G] AUTO GAIN", self.agc),
                ]),
                Line::from(Span::styled(
                    " Mic → high-pass / AEC3 / NS / AGC2 → Snacka → encrypted UDP",
                    Style::default().fg(DIM),
                )),
            ])
            .block(panel(" VOICE ENGINE ")),
            rows[3],
        );
        frame.render_widget(
            Paragraph::new(
                self.logs
                    .iter()
                    .map(|line| Line::from(format!(" › {line}")))
                    .collect::<Vec<_>>(),
            )
            .style(Style::default().fg(DIM))
            .block(panel(" SESSION LOG "))
            .wrap(Wrap { trim: true }),
            rows[4],
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    " M mute   ↑↓ volume   ←→ bitrate   A/N/G processing   Q / Esc hang up",
                    Style::default().fg(GOLD),
                )),
                Line::from(Span::styled(
                    " Snacka experimental codec • 48 kHz mono • 20 ms frames",
                    Style::default().fg(DIM),
                )),
            ]),
            rows[5],
        );
    }

    fn status(&self) -> &'static str {
        if self.connected {
            "● CONNECTED"
        } else if self.registered {
            "◌ WAITING FOR PEER"
        } else {
            "◌ CONNECTING"
        }
    }

    fn peer_card(&self, frame: &mut Frame, area: Rect, local: bool) {
        let title = if local {
            " YOU / MICROPHONE "
        } else {
            " PEER / PLAYBACK "
        };
        let block = panel(title);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
        let level = if local {
            self.local_level
        } else {
            self.remote_level
        };
        let label = if local && self.muted {
            "MUTED".to_owned()
        } else {
            format!("{:.0} dBFS", 20.0 * level.max(0.00001).log10())
        };
        frame.render_widget(
            Gauge::default()
                .ratio(f64::from(meter(level)))
                .label(label)
                .gauge_style(Style::default().fg(if local && self.muted { GOLD } else { CYAN })),
            rows[0],
        );
        frame.render_widget(
            Paragraph::new(if local {
                self.local.as_str()
            } else {
                self.remote.as_str()
            })
            .style(Style::default().fg(DIM)),
            rows[1],
        );
        frame.render_widget(
            Paragraph::new(if local {
                self.input.as_str()
            } else {
                self.output.as_str()
            }),
            rows[2],
        );
        frame.render_widget(
            Paragraph::new(if local {
                "Capture processing before encoding".into()
            } else {
                format!("Playback volume {:.0}%", self.gain * 100.0)
            })
            .style(Style::default().fg(DIM)),
            rows[3],
        );
    }
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
}

// A logarithmic -60..0 dBFS scale keeps ordinary speech visible, while the
// numeric label reports the actual peak rather than the display scale.
fn meter(peak: f32) -> f32 {
    ((20.0 * peak.max(0.000001).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}
fn toggle(label: &str, enabled: bool) -> Span<'_> {
    Span::styled(
        format!(" {label} {}", if enabled { "ON" } else { "OFF" }),
        Style::default().fg(if enabled { CYAN } else { DIM }),
    )
}

pub struct Terminal(ratatui::DefaultTerminal);
impl Terminal {
    pub fn enter() -> io::Result<Option<Self>> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Ok(None);
        }
        match ratatui::try_init() {
            Ok(terminal) => Ok(Some(Self(terminal))),
            Err(error) => {
                ratatui::restore();
                Err(error)
            }
        }
    }
    pub fn draw(&mut self, state: &Dashboard) -> io::Result<()> {
        self.0.draw(|frame| state.draw(frame))?;
        Ok(())
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn renders_call_and_compact_layouts_without_audio_or_network() {
        for (width, height) in [(100, 34), (64, 25), (40, 12)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut state = Dashboard::new("DIRECT IPv6", "[::1]:5000".into(), "[::1]:5001".into());
            state.connected = true;
            state.log("Peer authenticated — voice link established");
            terminal.draw(|frame| state.draw(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(text.contains("CONNECTED"));
            assert!(text.contains("AEC") || text.contains("ECHO CANCEL"));
        }
    }
}
