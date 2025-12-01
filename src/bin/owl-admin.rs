use owl_db::admin::AdminCli;

fn main() {
    if let Err(e) = AdminCli::run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
