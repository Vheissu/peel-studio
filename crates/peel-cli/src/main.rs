//! `peel`: headless project tools.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    eprintln!(
        "usage: {} <command>",
        args.first().map_or("peel", String::as_str)
    );
}
