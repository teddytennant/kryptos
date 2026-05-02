use std::process;

fn main() {
    kryptos::core::logging::init();
    let exit = kryptos::ui::run();
    process::exit(exit.value());
}
