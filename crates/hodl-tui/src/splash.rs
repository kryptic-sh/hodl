//! One-shot splash animation played at TUI launch via hjkl-splash.
//!
//! The cursor traces the H, O, D, L letterforms over the existing figlet
//! "ANSI Regular" art block, then transitions to the lock / onboarding
//! screen. Any keypress aborts immediately. Disable with `[ui] splash =
//! false` in `~/.config/hodl/config.toml`.

use std::time::Duration;

use anyhow::Result;
use crossterm::event;
use hjkl_splash::{Layout, Splash, default_trail_color};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::style::{Color, Style};

/// Embedded art block. Single source of truth — apps/hodl re-uses this for
/// the --help banner.
pub const ART: &str = include_str!("splash/art.txt");

/// Number of rows in the art block.
pub const ROWS: u16 = 5;

/// Number of cols in the art block.
pub const COLS: u16 = 33;

/// Cursor path tracing H, O, D, L letterforms.
///
/// Art layout (5 rows × 33 cols). Each entry is (row, col, letter_char).
/// Non-space glyph positions come from the ANSI Regular figlet art:
///   H: left-vert cols 0-1 rows 0-4; crossbar row 2 cols 0-6; right-vert cols 5-6
///   O: outer ring cols 8-15 rows 0-4
///   D: left-vert cols 17-18; rounded right bulge cols 22-23
///   L: vert cols 25-26 rows 0-4; bottom bar cols 25-32 row 4
///
/// Trace order per letter matches the hjkl preset style.
#[rustfmt::skip]
pub const PATH: &[(u8, u8, char)] = &[
    // H: left vertical top→bottom
    (0, 0, 'h'), (0, 1, 'h'),
    (1, 0, 'h'), (1, 1, 'h'),
    (2, 0, 'h'), (2, 1, 'h'),
    (3, 0, 'h'), (3, 1, 'h'),
    (4, 0, 'h'), (4, 1, 'h'),
    // H: crossbar left→right (row 2)
    (2, 2, 'h'), (2, 3, 'h'), (2, 4, 'h'), (2, 5, 'h'), (2, 6, 'h'),
    // H: right vertical bottom→top
    (4, 5, 'h'), (4, 6, 'h'),
    (3, 5, 'h'), (3, 6, 'h'),
    (1, 5, 'h'), (1, 6, 'h'),
    (0, 5, 'h'), (0, 6, 'h'),
    // O: top row left→right (row 0, cols 8-15)
    (0, 8, 'o'), (0, 9, 'o'),
    (0, 10, 'o'), (0, 11, 'o'), (0, 12, 'o'), (0, 13, 'o'),
    (0, 14, 'o'), (0, 15, 'o'),
    // O: right vertical top→bottom
    (1, 14, 'o'), (1, 15, 'o'),
    (2, 14, 'o'), (2, 15, 'o'),
    (3, 14, 'o'), (3, 15, 'o'),
    // O: bottom row right→left (row 4, cols 9-15)
    (4, 14, 'o'), (4, 15, 'o'),
    (4, 13, 'o'), (4, 12, 'o'), (4, 11, 'o'), (4, 10, 'o'),
    (4, 9, 'o'),
    // O: left vertical bottom→top
    (3, 8, 'o'), (3, 9, 'o'),
    (2, 8, 'o'), (2, 9, 'o'),
    (1, 8, 'o'), (1, 9, 'o'),
    // D: left vertical top→bottom
    (0, 17, 'd'), (0, 18, 'd'),
    (1, 17, 'd'), (1, 18, 'd'),
    (2, 17, 'd'), (2, 18, 'd'),
    (3, 17, 'd'), (3, 18, 'd'),
    (4, 17, 'd'), (4, 18, 'd'),
    // D: top row left→right (row 0, cols 19-23)
    (0, 19, 'd'), (0, 20, 'd'), (0, 21, 'd'),
    (0, 22, 'd'), (0, 23, 'd'),
    // D: right vertical top→bottom (cols 22-23)
    (1, 22, 'd'), (1, 23, 'd'),
    (2, 22, 'd'), (2, 23, 'd'),
    (3, 22, 'd'), (3, 23, 'd'),
    // D: bottom row right→left (row 4, cols 17-23)
    (4, 22, 'd'), (4, 23, 'd'),
    (4, 21, 'd'), (4, 20, 'd'), (4, 19, 'd'),
    (4, 18, 'd'), (4, 17, 'd'),
    // L: vertical top→bottom
    (0, 25, 'l'), (0, 26, 'l'),
    (1, 25, 'l'), (1, 26, 'l'),
    (2, 25, 'l'), (2, 26, 'l'),
    (3, 25, 'l'), (3, 26, 'l'),
    (4, 25, 'l'), (4, 26, 'l'),
    // L: bottom bar left→right (row 4, cols 27-32)
    (4, 27, 'l'), (4, 28, 'l'), (4, 29, 'l'),
    (4, 30, 'l'), (4, 31, 'l'), (4, 32, 'l'),
];

/// Run the splash to completion (or until the user presses any key).
/// Returns immediately if `enabled == false`.
pub fn run<B: Backend>(terminal: &mut Terminal<B>, enabled: bool) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    if !enabled {
        return Ok(());
    }

    let mut splash = Splash::new(ART, PATH);
    const TICKS: u64 = 30;
    const TICK_MS: u64 = 50;

    for _ in 0..TICKS {
        splash.advance();

        terminal.draw(|f| {
            let area = f.area();
            let layout = Layout::centered(area.width, area.height, ROWS, COLS);
            let buf = f.buffer_mut();

            for cell in splash.cells(layout) {
                use hjkl_splash::CellKind;
                let style = match cell.kind {
                    CellKind::Art => Style::default().fg(Color::DarkGray),
                    CellKind::Trail { age } => {
                        let rgb = default_trail_color(age);
                        Style::default().fg(Color::Rgb(rgb.0, rgb.1, rgb.2))
                    }
                    CellKind::Cursor => Style::default().fg(Color::White),
                };

                if cell.x < buf.area.width && cell.y < buf.area.height {
                    buf[(cell.x, cell.y)].set_char(cell.ch).set_style(style);
                }
            }
        })?;

        if event::poll(Duration::from_millis(TICK_MS))? {
            // Any event — drain and abort.
            let _ = event::read();
            break;
        }
    }

    // Clear the splash frame before the caller's screen takes over.
    terminal.clear()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hjkl_splash::{Layout, Splash};

    #[test]
    fn splash_constructs_without_panic() {
        let _splash = Splash::new(ART, PATH);
    }

    #[test]
    fn splash_advance_runs_n_times() {
        let mut splash = Splash::new(ART, PATH);
        for _ in 0..50 {
            splash.advance();
        }
        assert_eq!(splash.tick(), 50);
    }

    #[test]
    fn splash_cells_yields_entries() {
        let mut splash = Splash::new(ART, PATH);
        splash.advance();
        let layout = Layout::centered(120, 40, ROWS, COLS);
        let cells: Vec<_> = splash.cells(layout).collect();
        assert!(!cells.is_empty(), "cells() should yield at least one entry");
    }

    #[test]
    fn path_covers_all_four_letters() {
        // Verify at least one entry per letter.
        let has_h = PATH.iter().any(|(_, _, ch)| *ch == 'h');
        let has_o = PATH.iter().any(|(_, _, ch)| *ch == 'o');
        let has_d = PATH.iter().any(|(_, _, ch)| *ch == 'd');
        let has_l = PATH.iter().any(|(_, _, ch)| *ch == 'l');
        assert!(has_h, "PATH missing H entries");
        assert!(has_o, "PATH missing O entries");
        assert!(has_d, "PATH missing D entries");
        assert!(has_l, "PATH missing L entries");
    }

    #[test]
    fn path_len_in_expected_range() {
        assert!(
            PATH.len() >= 60 && PATH.len() <= 120,
            "PATH has {} entries; expected 60–120",
            PATH.len()
        );
    }
}
