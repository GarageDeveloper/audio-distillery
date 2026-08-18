/// Render a track file name (without extension) from a template.
///
/// Supported tokens: `{n}` (track number, zero-padded to the width of the
/// track count, min 2), `{title}` / `{titre}` (track title), `{source}`
/// (source file stem). The result is sanitized for cross-platform file names.
pub fn render_track_filename(
    template: &str,
    n: usize,
    total: usize,
    title: &str,
    source_stem: &str,
) -> String {
    let width = total.to_string().len().max(2);
    let num = format!("{n:0width$}");
    let out = template
        .replace("{n}", &num)
        .replace("{title}", title)
        .replace("{titre}", title)
        .replace("{source}", source_stem);
    sanitize_filename(&out)
}

/// Replace characters that are invalid on at least one target OS.
pub fn sanitize_filename(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = replaced.trim().trim_end_matches(['.', ' ']).trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_default_template() {
        assert_eq!(
            render_track_filename("{n} - {title}", 3, 12, "Intro", "concert"),
            "03 - Intro"
        );
    }

    #[test]
    fn pads_to_total_width() {
        assert_eq!(
            render_track_filename("{n} - {title}", 7, 120, "Solo", "x"),
            "007 - Solo"
        );
    }

    #[test]
    fn supports_source_and_french_alias() {
        assert_eq!(
            render_track_filename("{source} {n} {titre}", 1, 5, "A", "live"),
            "live 01 A"
        );
    }

    #[test]
    fn sanitizes_invalid_characters() {
        assert_eq!(
            render_track_filename("{n} - {title}", 1, 1, "AC/DC: Live?", "x"),
            "01 - AC_DC_ Live_"
        );
    }

    #[test]
    fn empty_result_falls_back() {
        assert_eq!(sanitize_filename("   ..."), "Untitled");
    }
}
