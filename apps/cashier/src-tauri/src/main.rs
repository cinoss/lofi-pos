#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = env::args().collect();
    if args.get(1).is_some_and(|a| a == "seed-owner") {
        if let Err(e) = seed_owner_cmd(&args[2..]) {
            eprintln!("seed-owner failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    if args.get(1).is_some_and(|a| a == "eod-now") {
        let day = args.get(2).cloned();
        if let Err(e) = cashier_lib::cli::run_eod_now(day) {
            eprintln!("eod-now failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    cashier_lib::run();
}

fn seed_owner_cmd(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use cashier_lib::acl::Role;
    use cashier_lib::auth::pin::hash_pin;
    use cashier_lib::store::master::Master;
    use std::path::PathBuf;

    let name = args.first().cloned().unwrap_or_else(|| "Owner".into());
    let pin = args
        .get(1)
        .cloned()
        .ok_or("Usage: cashier seed-owner <name> <pin>  (PIN must be >= 6 chars)")?;

    let identifier = "com.lofi-pos.cashier";
    let db_path: PathBuf = if cfg!(target_os = "macos") {
        let home = env::var("HOME")?;
        PathBuf::from(home)
            .join("Library/Application Support")
            .join(identifier)
    } else if cfg!(target_os = "windows") {
        let appdata = env::var("APPDATA")?;
        PathBuf::from(appdata).join(identifier)
    } else {
        let home = env::var("HOME")?;
        PathBuf::from(home).join(".local/share").join(identifier)
    }
    .join("master.db");

    if !db_path.exists() {
        return Err(
            format!("master.db not found at {db_path:?} - start the app once first.").into(),
        );
    }
    let master = Master::open(&db_path)?;
    let pin_hash = hash_pin(&pin)?;
    let id = master.create_staff(&name, &pin_hash, Role::Owner, None)?;
    println!("Seeded owner staff (id={id}, name={name:?})  Login PIN: {pin}");
    Ok(())
}
