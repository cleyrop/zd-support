use anyhow::Result;
use clap::ColorChoice;
use serde_json::Value;
use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthChar;

struct Style(bool);

impl Style {
    fn row(&self, color: &str, plain: &str, styled: &str, width: usize) -> String {
        // Measure the original text, never the ANSI escape sequences.
        let used: usize = plain.chars().map(|c| c.width().unwrap_or(0)).sum();
        let padding = " ".repeat((width - 4).saturating_sub(used));
        let border = self.paint(color, "│");
        format!("{border} {styled}{padding} {border}")
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.0 {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    // Keep the original Markdown readable, including when colors are disabled.
    fn inline(&self, line: &str) -> String {
        let mut result = String::new();
        let mut rest = line;
        while let Some(start) = rest.find('`') {
            let Some(end) = rest[start + 1..].find('`') else {
                break;
            };
            let end = start + end + 2;
            result.push_str(&rest[..start]);
            result.push_str(&self.paint("33", &rest[start..end]));
            rest = &rest[end..];
        }
        result.push_str(rest);
        result
    }
}

fn presentation(color: ColorChoice) -> (Style, usize) {
    let color = match color {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
                && std::env::var("TERM").as_deref() != Ok("dumb")
        }
    };
    let style = Style(color);
    let width = terminal_size::terminal_size()
        .map(|(w, _)| usize::from(w.0))
        .unwrap_or(100)
        .clamp(20, 120);
    (style, width)
}

pub fn display(value: &Value, color: ColorChoice) -> Result<()> {
    let (style, width) = presentation(color);
    let comments = value["comments"].as_array().expect("comments array");
    let users = value["users"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let me = &value["current_user"];
    let mut out = io::BufWriter::new(io::stdout().lock());
    writeln!(
        out,
        "{}",
        style.paint(
            "1",
            &format!(
                "Ticket #{} · {} messages",
                safe(&value["request_id"]),
                comments.len()
            )
        )
    )?;
    writeln!(
        out,
        "{}",
        style.paint("2", "Du plus ancien au plus récent · dates en UTC")
    )?;
    if comments.is_empty() {
        writeln!(out, "Aucun commentaire public.")?;
    }
    for (index, comment) in comments.iter().enumerate() {
        let author_id = comment["author_id"].as_u64();
        let is_me = author_id.is_some() && author_id == me["id"].as_u64();
        let author = if is_me {
            Some(me)
        } else {
            users
                .iter()
                .find(|user| author_id.is_some() && user["id"].as_u64() == author_id)
        };
        let is_support = author.is_some_and(|user| {
            user["agent"] == true || matches!(user["role"].as_str(), Some("agent" | "admin"))
        });
        let (label, color) = if is_me {
            ("VOUS", "36")
        } else if is_support {
            ("SUPPORT", "32")
        } else {
            ("PARTICIPANT", "35")
        };
        let name = author
            .filter(|user| {
                user["name"]
                    .as_str()
                    .is_some_and(|name| !name.trim().is_empty())
            })
            .map(|user| crate::table_text(&user["name"]))
            .unwrap_or_else(|| format!("Auteur #{}", safe(&comment["author_id"])));
        let last = if index + 1 == comments.len() {
            " · DERNIER MESSAGE"
        } else {
            ""
        };
        let heading = format!("{}/{}  {label} · {name}{last}", index + 1, comments.len());
        writeln!(
            out,
            "\n{}",
            style.paint(color, &format!("┌{}┐", "─".repeat(width - 2)))
        )?;
        for line in wrap(&heading, width - 4) {
            writeln!(
                out,
                "{}",
                style.row(
                    color,
                    &line,
                    &style.paint(&format!("1;{color}"), &line),
                    width
                )
            )?;
        }
        let date = crate::table_text(&comment["created_at"]).replace('T', " ");
        let date = if let Some(date) = date.strip_suffix('Z') {
            format!("{date} UTC")
        } else {
            date
        };
        for line in wrap(
            &format!("{date} · commentaire #{}", safe(&comment["id"])),
            width - 4,
        ) {
            writeln!(
                out,
                "{}",
                style.row(color, &line, &style.paint("2", &line), width)
            )?;
        }
        writeln!(out, "{}", style.row(color, "", "", width))?;
        write_body(&mut out, &style, color, &safe(&comment["body"]), width)?;
        writeln!(
            out,
            "{}",
            style.paint(color, &format!("└{}┘", "─".repeat(width - 2)))
        )?;
    }
    out.flush()?;
    Ok(())
}

fn write_body(
    out: &mut impl Write,
    style: &Style,
    color: &str,
    body: &str,
    width: usize,
) -> Result<()> {
    let mut fence: Option<char> = None;
    for original in body.lines() {
        let trimmed = original.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some('`')
        } else if trimmed.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        let is_code = fence.is_some() || marker.is_some();
        if let Some(marker) = marker {
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
        }
        for line in wrap(original, width - 4) {
            let body_line = if is_code {
                style.paint("33", &line)
            } else if trimmed.starts_with('>') {
                style.paint("2", &line)
            } else if trimmed.starts_with("# ") || trimmed.starts_with("## ") {
                style.paint("1", &line)
            } else {
                style.inline(&line)
            };
            writeln!(out, "{}", style.row(color, &line, &body_line, width))?;
        }
    }
    Ok(())
}

fn date(value: &Value) -> String {
    let value = crate::table_text(value).replace('T', " ");
    match value.strip_suffix('Z') {
        Some(value) => format!("{value} UTC"),
        None => value,
    }
}

fn write_lines(
    out: &mut impl Write,
    style: &Style,
    color: &str,
    text: &str,
    emphasis: &str,
    width: usize,
) -> Result<()> {
    for line in wrap(text, width - 4) {
        writeln!(
            out,
            "{}",
            style.row(color, &line, &style.paint(emphasis, &line), width)
        )?;
    }
    Ok(())
}

pub fn display_request(request: &Value, color: ColorChoice) -> Result<()> {
    let (style, width) = presentation(color);
    let status = crate::table_text(&request["status"]);
    let color = match status.as_str() {
        "new" => "36",
        "open" => "34",
        "pending" | "hold" => "33",
        "solved" => "32",
        "closed" => "2",
        _ => "35",
    };
    let id = crate::table_text(&request["id"]);
    let mut out = io::BufWriter::new(io::stdout().lock());
    writeln!(
        out,
        "{}",
        style.paint(color, &format!("┌{}┐", "─".repeat(width - 2)))
    )?;
    write_lines(
        &mut out,
        &style,
        color,
        &format!("TICKET #{id} · {}", status.to_uppercase()),
        &format!("1;{color}"),
        width,
    )?;
    write_lines(
        &mut out,
        &style,
        color,
        &crate::table_text(&request["subject"]),
        "1",
        width,
    )?;
    writeln!(out, "{}", style.row(color, "", "", width))?;
    for (field, label) in [("created_at", "Créé le"), ("updated_at", "Mis à jour le")] {
        if request[field].is_string() {
            write_lines(
                &mut out,
                &style,
                color,
                &format!("{label:13} {}", date(&request[field])),
                "2",
                width,
            )?;
        }
    }
    for (field, label) in [("priority", "Priorité"), ("type", "Type")] {
        if let Some(value) = request[field].as_str().filter(|v| !v.is_empty()) {
            let emphasis = if field == "priority" && matches!(value, "high" | "urgent") {
                "1;31"
            } else {
                "0"
            };
            write_lines(
                &mut out,
                &style,
                color,
                &format!("{label:13} {}", crate::table_text(&request[field])),
                emphasis,
                width,
            )?;
        }
    }
    writeln!(
        out,
        "{}",
        style.paint(color, &format!("├{}┤", "─".repeat(width - 2)))
    )?;
    write_lines(&mut out, &style, color, "DESCRIPTION INITIALE", "1", width)?;
    writeln!(out, "{}", style.row(color, "", "", width))?;
    let body = request["description"]
        .as_str()
        .filter(|v| !v.trim().is_empty())
        .map(|_| safe(&request["description"]))
        .unwrap_or_else(|| "Aucune description disponible.".into());
    write_body(&mut out, &style, color, &body, width)?;
    writeln!(
        out,
        "{}",
        style.paint(color, &format!("├{}┤", "─".repeat(width - 2)))
    )?;
    write_lines(
        &mut out,
        &style,
        color,
        &format!("Tous les échanges : zd-support comments {id}"),
        "2",
        width,
    )?;
    writeln!(
        out,
        "{}",
        style.paint(color, &format!("└{}┘", "─".repeat(width - 2)))
    )?;
    out.flush()?;
    Ok(())
}

fn safe(value: &Value) -> String {
    crate::text(value)
        .chars()
        .filter(|c| !matches!(*c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .collect::<String>()
        .replace('\t', "    ")
}

// Wrap at word boundaries when possible, including wide Unicode characters.
// Long identifiers are continued, never truncated; existing newlines stay intact.
fn wrap(line: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = line;
    loop {
        let mut used = 0;
        let mut space = None;
        let mut split = None;
        for (index, c) in rest.char_indices() {
            used += c.width().unwrap_or(0);
            if used > width {
                split = Some(space.filter(|i| *i > 0).unwrap_or(index));
                break;
            }
            if c.is_whitespace() {
                space = Some(index);
            }
        }
        let Some(split) = split else {
            lines.push(rest.to_owned());
            break;
        };
        lines.push(rest[..split].to_owned());
        rest = rest[split..].trim_start();
    }
    lines
}
