//! App-side resize helper (sidebar-resize D6/D7): translate the physical cursor x to the
//! logical sidebar width fed to `renderer::Renderer::set_sidebar_width` — which clamps to
//! the Warp-style `[SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX]` band. This helper adds NO bound
//! of its own (the clamp is reused, not duplicated).

/// The logical sidebar width for a resize drag at physical cursor x. The sidebar's right
/// edge tracks the cursor, so the width is the cursor x converted to logical px (÷ scale).
/// Clamping to `[SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX]` is the renderer's job
/// (`set_sidebar_width`), so nothing is clamped here (D7/SD4).
pub fn resize_width_logical(cursor_x_physical: f64, scale: f64) -> f32 {
    if scale <= 0.0 {
        return cursor_x_physical as f32; // degenerate — avoid a divide-by-zero
    }
    (cursor_x_physical / scale) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_is_cursor_x_in_logical_px() {
        // At scale 1.0 the logical width equals the physical cursor x.
        assert_eq!(resize_width_logical(300.0, 1.0), 300.0);
        // At scale 2.0 (retina), a physical x of 600 is 300 logical px.
        assert_eq!(resize_width_logical(600.0, 2.0), 300.0);
        // A degenerate scale does not panic.
        assert_eq!(resize_width_logical(120.0, 0.0), 120.0);
    }
}
