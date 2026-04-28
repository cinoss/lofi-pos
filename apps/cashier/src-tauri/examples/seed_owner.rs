//! Seed an initial owner staff row into master.db.
//!
//! Usage:
//!   cargo run --example seed_owner -- "Boss" 999999
//!
//! Defaults: name="Owner", role="owner", PIN required (>= 6 digits).

use cashier_lib::acl::Role;
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::store::master::Master;
use std::env;
use std::path::PathBuf;

fn app_data_dir() -> PathBuf {
    // macOS: ~/Library/Application Support/com.lofi-pos.cashier
    // Linux: ~/.local/share/com.lofi-pos.cashier
    // Windows: %APPDATA%\com.lofi-pos.cashier
    let identifier = "com.lofi-pos.cashier";
    if cfg!(target_os = "macos") {
        let home = env::var("HOME").expect("HOME not set");
        PathBuf::from(home)
            .join("Library/Application Support")
            .join(identifier)
    } else if cfg!(target_os = "windows") {
        let appdata = env::var("APPDATA").expect("APPDATA not set");
        PathBuf::from(appdata).join(identifier)
    } else {
        let home = env::var("HOME").expect("HOME not set");
        PathBuf::from(home).join(".local/share").join(identifier)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let name = args.get(1).cloned().unwrap_or_else(|| "Owner".into());
    let pin = args.get(2).cloned().unwrap_or_else(|| {
        eprintln!("Usage: cargo run --example seed_owner -- <name> <pin>");
        eprintln!("PIN must be >= 6 characters.");
        std::process::exit(1);
    });

    let db_path = app_data_dir().join("master.db");
    if !db_path.exists() {
        eprintln!(
            "master.db not found at {db_path:?} - start the cashier app once first to create it."
        );
        std::process::exit(1);
    }

    let master = Master::open(&db_path)?;
    let pin_hash = hash_pin(&pin)?;
    let id = master.create_staff(&name, &pin_hash, Role::Owner, None)?;

    println!("Seeded owner staff (id={id}, name={name:?})");
    println!("Login with PIN: {pin}");
    Ok(())
}
