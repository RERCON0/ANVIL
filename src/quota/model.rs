//! What the quota worker knows about each provider.

/// Secondary windows stay hidden until the user ticks them.
pub fn window_visible_by_default(key: &str) -> bool {
    !matches!(key, "mcp" | "review")
}