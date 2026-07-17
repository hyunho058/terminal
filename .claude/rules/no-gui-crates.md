# Rule: no GUI/widget crates

**Constraint C2.** Rendering is pure wgpu/Metal with hand-rolled cells. This is
the project's reason to exist — do not undermine it.

## Prohibited

Do not add any GUI/widget toolkit: `egui`, `iced`, `druid`, `slint`, `gtk`,
`gpui`, `dioxus`, `tao`, `muda`-as-UI, or any crate that owns layout/widgets.

## Allowed (explicit exceptions)

- `winit` — windowing + events + IME (not a widget toolkit)
- `objc2` / `objc2-*` — macOS platform bindings (clipboard, menus, etc.)
- `wgpu`, `swash`, `bytemuck`, `raw-window-handle` — the render stack

## If you think you need a GUI crate

Stop and surface it to the human. The answer is almost always: draw it yourself
on the cell grid, or add a small `objc2` call for a native macOS affordance.
