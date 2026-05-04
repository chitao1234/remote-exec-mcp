pub fn preview_text(raw: &str, limit: usize) -> String {
    let mut preview = raw.chars().take(limit).collect::<String>();
    if raw.chars().count() > limit {
        preview.push_str("...");
    }
    preview
}
