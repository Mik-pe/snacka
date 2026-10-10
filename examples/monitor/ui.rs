// SPDX-License-Identifier: MIT
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph, Sparkline, Wrap},
};
use snacka::Config;

const ACCENT: Color = Color::Cyan;
const RAW: Color = Color::Yellow;
const DIM: Color = Color::DarkGray;

pub struct View<'a> {
    pub config: Config,
    pub raw: bool,
    pub gain: f32,
    pub buffer_ms: usize,
    pub input: &'a str,
    pub output: &'a str,
    pub queued_ms: f64,
    pub capture_drops: usize,
    pub playback_gaps: usize,
    pub playback_drops: usize,
    pub clipped: usize,
    pub elapsed: f64,
    pub encoded_bytes: usize,
    pub samples: usize,
}

pub struct Ui {
    pub peak: f32,
    pub muted: bool,
    pub help: bool,
    pub notice: String,
    history: [u64; 96],
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            peak: 0.0,
            muted: false,
            help: false,
            notice:
                "Put on headphones, start quietly, then speak. Space compares raw and codec audio."
                    .into(),
            history: [0; 96],
        }
    }
}

fn encoded_kilobytes_per_second(view: &View<'_>) -> f64 {
    if view.samples == 0 {
        0.0
    } else {
        view.encoded_bytes as f64 * f64::from(snacka::SAMPLE_RATE) / view.samples as f64 / 1000.0
    }
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(DIM))
        .title(title)
}

impl Ui {
    pub fn tick(&mut self) {
        self.history.rotate_left(1);
        self.history[95] = (self.peak * 1000.0) as u64;
        self.peak *= 0.8;
    }

    pub fn render(&self, frame: &mut Frame, view: &View<'_>) {
        let area = frame.area();
        if area.width < 54 || area.height < 23 {
            self.compact(frame, view);
            return;
        }
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" SNACKA ", Style::new().bg(ACCENT).fg(Color::Black).bold()),
                Span::raw("  Voice codec listening lab"),
                Span::styled(
                    format!(
                        "   LIVE {:02}:{:02}",
                        view.elapsed as u64 / 60,
                        view.elapsed as u64 % 60
                    ),
                    Style::new().fg(DIM),
                ),
            ])),
            rows[0],
        );
        let (mode, color) = if self.muted {
            ("MUTED", DIM)
        } else if view.raw {
            ("RAW REFERENCE", RAW)
        } else {
            ("SNACKA CODEC", ACCENT)
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!(" {mode} "),
                        Style::new().bg(color).fg(Color::Black).bold(),
                    ),
                    Span::raw("  [Space] compare   [M] mute"),
                ]),
                Line::from(" Mic  ->  encoder  ->  packet  ->  decoder  ->  headphones")
                    .dark_gray(),
            ]),
            rows[1],
        );

        let meters = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(rows[2]);
        frame.render_widget(
            Gauge::default()
                .block(panel(" Microphone "))
                .gauge_style(Style::new().fg(if self.peak > 0.95 {
                    Color::Red
                } else {
                    Color::Green
                }))
                .ratio(f64::from(self.peak.clamp(0.0, 1.0)))
                .label(format!("{:.0}% peak", self.peak * 100.0)),
            meters[0],
        );
        frame.render_widget(
            Sparkline::default()
                .block(panel(" Voice activity / last 10s "))
                .data(&self.history[..])
                .max(1000)
                .style(Style::new().fg(ACCENT)),
            meters[1],
        );

        let controls = Layout::horizontal([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(rows[3]);
        let bitrate = panel(" Bitrate ");
        let inner = bitrate.inner(controls[0]);
        frame.render_widget(bitrate, controls[0]);
        let rate_rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);
        frame.render_widget(
            Paragraph::new(format!(" {} kb/s", view.config.bitrate() as f32 / 1000.0))
                .bold()
                .cyan(),
            rate_rows[0],
        );
        frame.render_widget(
            Paragraph::new(format!(
                " {:.2} kB/s avg",
                encoded_kilobytes_per_second(view)
            )),
            rate_rows[1],
        );
        frame.render_widget(
            Gauge::default()
                .ratio(f64::from(view.config.bitrate() - 12_000) / 20_000.0)
                .gauge_style(Style::new().fg(ACCENT))
                .label("12 - 32 kb/s"),
            rate_rows[2],
        );
        frame.render_widget(
            Paragraph::new(" Left / Right or - / +").dark_gray(),
            rate_rows[3],
        );

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    " {} ms frames",
                    view.config.frame_duration().milliseconds()
                ))
                .bold()
                .cyan(),
                Line::from(format!(" {} ms prebuffer", view.buffer_ms)),
                Line::from(" [F] frame  [B] buffer").dark_gray(),
            ])
            .block(panel(" Timing ")),
            controls[1],
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    " {:.0}%{}",
                    view.gain * 100.0,
                    if self.muted { " / muted" } else { "" }
                ))
                .bold()
                .cyan(),
                Line::from(" Headphones recommended").dark_gray(),
                Line::from(" Up / Down   [M] mute").dark_gray(),
            ])
            .block(panel(" Monitor volume ")),
            controls[2],
        );

        let bottom = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(rows[4]);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(" IN  ", Style::new().fg(ACCENT)),
                    Span::raw(view.input),
                ]),
                Line::from(vec![
                    Span::styled(" OUT ", Style::new().fg(ACCENT)),
                    Span::raw(view.output),
                ]),
                Line::from(" Mono / 48 kHz   |   No audio saved").dark_gray(),
            ])
            .block(panel(" Devices "))
            .wrap(Wrap { trim: false }),
            bottom[0],
        );
        let trouble = view.capture_drops + view.playback_gaps + view.playback_drops + view.clipped;
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(" {:.1} ms queued", view.queued_ms)).fg(if trouble > 0 {
                    RAW
                } else {
                    Color::Green
                }),
                Line::from(format!(
                    " Gaps {}   Drops {}",
                    view.playback_gaps,
                    view.capture_drops + view.playback_drops
                )),
                Line::from(format!(" Mic clipped {} samples", view.clipped)).dark_gray(),
            ])
            .block(panel(" Stream health / samples ")),
            bottom[1],
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(" [Space] A/B  ", Style::new().fg(ACCENT).bold()),
                    Span::raw("[?] help   [Q / Esc] quit"),
                ]),
                Line::from(self.notice.as_str()).dark_gray(),
            ])
            .wrap(Wrap { trim: false }),
            rows[5],
        );
        if self.help {
            self.render_help(frame);
        }
    }

    fn compact(&self, frame: &mut Frame, view: &View<'_>) {
        let mode = if self.muted {
            "MUTED"
        } else if view.raw {
            "RAW REFERENCE"
        } else {
            "SNACKA CODEC"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("SNACKA / Voice codec listening lab")
                    .bold()
                    .cyan(),
                Line::from(format!("{mode}  |  Space: compare")),
                Line::from(format!(
                    "{} kb/s  |  {} ms  |  volume {:.0}%",
                    view.config.bitrate() / 1000,
                    view.config.frame_duration().milliseconds(),
                    view.gain * 100.0
                )),
                Line::from(format!(
                    "48 kHz mono | {:.2} kB/s avg",
                    encoded_kilobytes_per_second(view)
                )),
                Line::from(format!(
                    "Mic {:.0}%  |  buffer {} ms",
                    self.peak * 100.0,
                    view.buffer_ms
                )),
                Line::from("Left/Right: bitrate  Up/Down: volume"),
                Line::from("F: frame  B: buffer  M: mute  Q: quit"),
                Line::from("?: help   Use headphones at low volume."),
                Line::from(self.notice.as_str()).dark_gray(),
            ])
            .wrap(Wrap { trim: false }),
            frame.area(),
        );
        if self.help {
            self.render_help(frame);
        }
    }

    fn render_help(&self, frame: &mut Frame) {
        let area = frame.area();
        let popup = Rect::new(
            area.x + area.width / 10,
            area.y + area.height / 10,
            area.width * 8 / 10,
            area.height * 8 / 10,
        );
        frame.render_widget(Clear, popup);
        let guide = if popup.height < 23 || popup.width < 68 {
            "48 kHz = 48,000 samples each second.\nkb/s = compressed bits each second.\n12 kb/s = 1.5 kB/s (90 kB/min).\nAvg: packet data; excludes network.\nSpace: raw/codec (same capture).\nLeft/Right: bitrate Up/Down: volume\nF: frame B: buffer M: mute Q: quit\n?: close 12 kb/s can sound rough."
        } else {
            "48 kHz = 48,000 mono samples each second.\nkb/s = compressed bits each second; 12 kb/s = 1.5 kB/s.\nAvg is packet bytes per second of processed audio since startup.\nPacket headers count; network and encryption do not.\n12 kb/s is experimental: listen for distortion; compare 24 and 32.\n\nSpace     Compare Snacka and raw audio\nLeft/Right or -/+   Change bitrate by 1 kb/s\nUp/Down   Change playback volume by 5%\nF         Switch 10 / 20 ms frames (restarts codec)\nB         Cycle 20 / 40 / 80 / 120 / 200 ms buffer\nM         Mute playback (capture continues)\nQ / Ctrl-C   Quit and restore terminal\n? / Esc      Close this help\n\nBoth modes share device/OS processing and sample-rate conversion.\nRaw bypasses Snacka with the same 2.5 ms transform delay.\nHigher buffering can help gaps, at the cost of delay.\nActual latency also includes capture and device buffers.\nFor devices: --list-devices, --input N, --output N.\nNo audio is saved to disk."
        };
        frame.render_widget(
            Paragraph::new(guide)
                .block(panel(" Listening guide / [?] close "))
                .wrap(Wrap { trim: true }),
            popup,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn displayed_data_tracks_generated_packets_including_rate_changes() {
        let mut processor =
            super::super::processor::Processor::new(Config::new(12_000).unwrap(), true);
        let input = [0.0; snacka::FRAME_SAMPLES];
        let mut output = input;
        processor.process(&input, &mut output).unwrap();
        let mut view = View {
            config: processor.config,
            raw: true,
            gain: 0.5,
            buffer_ms: 40,
            input: "Microphone",
            output: "Headphones",
            queued_ms: 0.0,
            capture_drops: 0,
            playback_gaps: 0,
            playback_drops: 0,
            clipped: 0,
            elapsed: 100.0,
            encoded_bytes: processor.bytes,
            samples: processor.samples,
        };
        assert_eq!(encoded_kilobytes_per_second(&view), 1.5);
        processor.set_bitrate(32_000).unwrap();
        processor.process(&input, &mut output).unwrap();
        view.config = processor.config;
        view.encoded_bytes = processor.bytes;
        view.samples = processor.samples;
        assert_eq!(encoded_kilobytes_per_second(&view), 2.75);
        view.samples = 0;
        assert_eq!(encoded_kilobytes_per_second(&view), 0.0);
    }

    #[test]
    fn listening_help_explains_units_and_preserves_exit_controls() {
        for (width, height) in [(100, 30), (60, 24), (48, 16)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let ui = Ui {
                help: true,
                ..Ui::default()
            };
            terminal.draw(|frame| ui.render_help(frame)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("samples each second"));
            assert!(text.contains("1.5 kB/s"));
            assert!(text.contains("network"));
            assert!(text.contains("Q:") || text.contains("Q / Ctrl-C"));
        }
    }

    #[test]
    fn dashboard_and_compact_layout_keep_controls_visible() {
        for (width, height) in [(100, 30), (60, 24), (54, 23), (48, 16)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let ui = Ui::default();
            let view = View {
                config: Config::default(),
                raw: true,
                gain: 0.5,
                buffer_ms: 40,
                input: "Test microphone",
                output: "Test headphones",
                queued_ms: 40.0,
                capture_drops: 0,
                playback_gaps: 0,
                playback_drops: 0,
                clipped: 0,
                elapsed: 2.0,
                encoded_bytes: 6000,
                samples: 96_000,
            };
            terminal.draw(|f| ui.render(f, &view)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            assert!(text.contains("RAW REFERENCE"));
            assert!(text.contains("SNACKA"));
            assert!(text.contains("volume") || text.contains("Volume"));
            assert!(text.contains("quit"));
            assert!(text.contains("3.00 kB/s avg"));
        }
    }
}
