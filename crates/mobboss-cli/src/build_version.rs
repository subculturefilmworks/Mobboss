pub fn format_version(tag: Option<&str>, base: &str, date: &str, commit: Option<&str>) -> String {
    let version = tag
        .filter(|tag| !tag.is_empty())
        .map(|tag| tag.strip_prefix('v').unwrap_or(tag).to_owned())
        .unwrap_or_else(|| format!("{base}-dev"));
    let commit = commit
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown");
    format!("v{version} · {date} · {commit}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_format() {
        assert_eq!(
            format_version(
                Some("v0.1.0-rc13"),
                "0.1.0",
                "2026-11-25 04:30 UTC",
                Some("02561d7\n")
            ),
            "v0.1.0-rc13 · 2026-11-25 04:30 UTC · 02561d7"
        );
    }
    #[test]
    fn missing_tag_and_git() {
        assert_eq!(
            format_version(None, "0.1.0", "2026-11-25 04:30 UTC", None),
            "v0.1.0-dev · 2026-11-25 04:30 UTC · unknown"
        );
    }
}
