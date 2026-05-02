use std::process;

fn main() {
    sigvim::core::logging::init();
    let exit = sigvim::ui::run();
    process::exit(exit.value());
}
