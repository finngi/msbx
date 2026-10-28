// Nothing outside tests uses these until tui.rs wires the event loop.
#[cfg_attr(not(test), allow(dead_code))]
mod app;
#[cfg_attr(not(test), allow(dead_code))]
mod model;
#[cfg_attr(not(test), allow(dead_code))]
mod ui;

fn main() {}
