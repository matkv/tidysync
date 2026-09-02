use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
// Taken through gtk rather than as its own dependency, so there is no way for
// the glib we call and the glib tray-icon links against to drift apart.
use gtk::glib;
use tokio::sync::mpsc;
use tracing::{debug, error, info};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::logging::TrayLogging;
use crate::watcher::{WatchState, WatcherControl};

/// How often the tray thread checks for menu clicks and status changes.
const REFRESH: Duration = Duration::from_millis(200);

const ICON_SIZE: u32 = 32;

/// Most recent log lines to show in the menu.
const RECENT_ROWS: usize = 10;

/// Index of the first recent row: status, separator, toggle, separator, header.
const RECENT_START: usize = 5;

/// Longest menu label before eliding; keeps the menu a sane width.
const MAX_LABEL: usize = 60;

/// Start the tray on its own thread.
///
/// GTK needs an event loop, and the tray must be built on whichever thread runs
/// it — but that thread does not have to be the main one. Keeping it off the
/// main thread lets `main` stay a normal `#[tokio::main]` async fn, so every
/// existing CLI path is untouched.
///
/// Returns once the tray is running; `quit` fires when the user picks Quit.
pub fn spawn(
    control: WatcherControl,
    syncthing_url: String,
    logging: TrayLogging,
    quit: mpsc::UnboundedSender<()>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("tray".to_owned())
        .spawn(move || {
            if let Err(err) = run(control, syncthing_url, &logging, &quit) {
                error!("Tray failed: {err:#}");
                // Without a tray there is no way to drive the app, so bring the
                // whole process down rather than watching on invisibly.
                let _ = quit.send(());
            }
        })
        .expect("failed to spawn tray thread")
}

fn run(
    control: WatcherControl,
    syncthing_url: String,
    logging: &TrayLogging,
    quit: &mpsc::UnboundedSender<()>,
) -> Result<()> {
    gtk::init().context("failed to initialise GTK for the tray icon")?;

    let status_item = MenuItem::new("Starting…", false, None);
    let toggle = CheckMenuItem::new("Watch enabled", true, control.is_enabled(), None);
    let recent_header = MenuItem::new("Recent", false, None);
    let open_ui = MenuItem::new("Open Syncthing UI", true, None);
    let open_log = MenuItem::new("Open log file", true, None);
    let clear_log = MenuItem::new("Clear log file", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    // muda has no way to hide a menu item, so rather than padding the menu with
    // blank rows these are inserted one at a time as the buffer fills. The ring
    // only ever grows, so items are never removed again.
    let recent_items: Vec<MenuItem> = (0..RECENT_ROWS)
        .map(|_| MenuItem::new("", false, None))
        .collect();
    let mut recent_shown = 0usize;

    let menu = Menu::new();
    menu.append_items(&[
        &status_item,
        &PredefinedMenuItem::separator(),
        &toggle,
        &PredefinedMenuItem::separator(),
        &recent_header,
        &PredefinedMenuItem::separator(),
        &open_ui,
        &open_log,
        &clear_log,
        &quit_item,
    ])
    .context("failed to build tray menu")?;

    // Menu is a cheap handle, so keeping a clone lets us add rows later while
    // the tray owns the menu itself.
    let menu_handle = menu.clone();

    // The tray icon must outlive the event loop; dropping it removes the icon.
    let tray = TrayIconBuilder::new()
        .with_tooltip("tidysync")
        .with_icon(icon_for(control.status().state)?)
        .with_menu(Box::new(menu))
        .build()
        .context("failed to create tray icon")?;

    let toggle_id = toggle.id().clone();
    let open_ui_id = open_ui.id().clone();
    let open_log_id = open_log.id().clone();
    let clear_log_id = clear_log.id().clone();
    let quit_id = quit_item.id().clone();

    let quit = quit.clone();
    let recent = logging.recent.clone();
    let log_path = logging.log_path.clone();
    let mut last_generation = u64::MAX;
    let mut last_state = None;
    let mut last_recent: Vec<String> = Vec::new();

    glib::timeout_add_local(REFRESH, move || {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == toggle_id {
                // The check item has already flipped itself; mirror it.
                let enabled = toggle.is_checked();
                info!("Watching {}", if enabled { "enabled" } else { "disabled" });
                control.set_enabled(enabled);
            } else if event.id == open_ui_id {
                if let Err(err) = open::that_detached(&syncthing_url) {
                    error!("Could not open {syncthing_url}: {err}");
                }
            } else if event.id == open_log_id {
                if let Err(err) = open::that_detached(&log_path) {
                    error!("Could not open {}: {err}", log_path.display());
                }
            } else if event.id == clear_log_id {
                match crate::logging::clear_log(&log_path, &recent) {
                    // Logged after clearing, so the menu shows why it emptied.
                    Ok(()) => info!("Log file cleared"),
                    Err(err) => error!("Could not clear the log file: {err:#}"),
                }
            } else if event.id == quit_id {
                debug!("Quit selected");
                let _ = quit.send(());
                gtk::main_quit();
                return glib::ControlFlow::Break;
            }
        }

        // The recent lines change independently of watcher status (device
        // connects, poll retries), so they get their own comparison.
        let lines = recent.lines();
        if lines != last_recent {
            // Match the number of visible rows to the number of lines. Normally
            // this only grows as the buffer fills; clearing the log is the one
            // thing that shrinks it.
            while recent_shown < lines.len() {
                if let Err(err) =
                    menu_handle.insert(&recent_items[recent_shown], RECENT_START + recent_shown)
                {
                    error!("Could not grow the recent list: {err}");
                    break;
                }
                recent_shown += 1;
            }

            while recent_shown > lines.len() {
                if let Err(err) = menu_handle.remove(&recent_items[recent_shown - 1]) {
                    error!("Could not shrink the recent list: {err}");
                    break;
                }
                recent_shown -= 1;
            }

            // Newest first reads better in a menu that hangs off the tray.
            for (row, line) in lines.iter().rev().enumerate().take(recent_shown) {
                recent_items[row].set_text(elide(line));
            }

            last_recent = lines;
        }

        let status = control.status();

        if status.generation != last_generation {
            last_generation = status.generation;

            status_item.set_text(status_line(&status));

            // Keep the checkbox honest if the watcher stopped on its own.
            let enabled = control.is_enabled();
            if toggle.is_checked() != enabled {
                toggle.set_checked(enabled);
            }

            if last_state != Some(status.state) {
                last_state = Some(status.state);
                match icon_for(status.state) {
                    Ok(icon) => {
                        if let Err(err) = tray.set_icon(Some(icon)) {
                            error!("Could not update tray icon: {err}");
                        }
                    }
                    Err(err) => error!("Could not build tray icon: {err:#}"),
                }
            }
        }

        glib::ControlFlow::Continue
    });

    info!("Tray running");
    gtk::main();

    Ok(())
}

fn status_line(status: &crate::watcher::Status) -> String {
    match (&status.last_error, status.state) {
        (Some(error), WatchState::Failed) => format!("Error: {}", elide(error)),
        (_, state) => format!("{} — {} moved", state.label(), status.moved),
    }
}

/// Menu labels are single-line and shouldn't stretch the menu across the screen;
/// an anyhow chain or a long path would do both.
fn elide(message: &str) -> String {
    let line = message.lines().next().unwrap_or_default();

    if line.chars().count() > MAX_LABEL {
        format!("{}…", line.chars().take(MAX_LABEL - 1).collect::<String>())
    } else {
        line.to_owned()
    }
}


/// Build the tray icon as raw RGBA.
///
/// Drawing it here rather than shipping a PNG keeps the binary self-contained
/// and avoids pulling in an image decoder for one 32×32 placeholder. Recolouring
/// per state then costs nothing.
fn icon_for(state: WatchState) -> Result<Icon> {
    let accent: [u8; 3] = match state {
        WatchState::Watching | WatchState::Scanning => [0x4c, 0x8d, 0xf6], // blue
        WatchState::Paused => [0x8a, 0x8a, 0x8a],                          // grey
        WatchState::Failed => [0xe0, 0x8b, 0x2c],                          // amber
    };

    let size = ICON_SIZE as i32;
    let mut rgba = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];

    let mut put = |x: i32, y: i32, colour: [u8; 4]| {
        if (0..size).contains(&x) && (0..size).contains(&y) {
            let i = ((y * size + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&colour);
        }
    };

    // Rounded square background.
    let margin = 2;
    let radius = 7;
    for y in margin..size - margin {
        for x in margin..size - margin {
            let dx = (margin + radius - x).max(x - (size - margin - 1 - radius)).max(0);
            let dy = (margin + radius - y).max(y - (size - margin - 1 - radius)).max(0);
            if dx * dx + dy * dy <= radius * radius {
                put(x, y, [accent[0], accent[1], accent[2], 0xff]);
            }
        }
    }

    // A downward arrow into a tray: the "move things into place" glyph.
    let white = [0xff, 0xff, 0xff, 0xff];
    let centre = size / 2;

    for y in 8..17 {
        for x in centre - 2..centre + 2 {
            put(x, y, white);
        }
    }
    for (step, y) in (17..23).enumerate() {
        let half = 6 - step as i32;
        for x in centre - half..centre + half {
            put(x, y, white);
        }
    }
    for x in 9..size - 9 {
        for y in 24..26 {
            put(x, y, white);
        }
    }

    Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).context("failed to build tray icon from pixels")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elide_keeps_short_lines_intact() {
        assert_eq!(elide("Moved report.pdf"), "Moved report.pdf");
    }

    #[test]
    fn elide_truncates_and_takes_only_the_first_line() {
        let long = "x".repeat(100);
        let elided = elide(&long);
        assert_eq!(elided.chars().count(), MAX_LABEL);
        assert!(elided.ends_with('…'));

        assert_eq!(elide("first\nsecond"), "first");
    }

    #[test]
    fn elide_counts_characters_not_bytes() {
        // Multi-byte input must not panic on a byte-boundary slice.
        let wide = "é".repeat(100);
        assert_eq!(elide(&wide).chars().count(), MAX_LABEL);
    }
}
