mod games;
mod openrgb;
mod words;

use std::env;

fn main()
{
    if let Err(err) = run() {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String>
{
    let mut args = env::args().skip(1);
    let command = args.next();
    let rest: Vec<String> = args.collect();
    match command.as_deref() {
        None => interactive_menu(),
        Some("list") => {
            list_games();
            Ok(())
        }
        Some("typing") => {
            run_game("typing", &rest)
        }
        Some("wordle") => {
            run_game("wordle", &rest)
        }
        Some("-h") | Some("--help") => {
            print_help();
            Ok(())
        }
        Some(other) => Err(format!("Unknown command '{other}'. Run with --help.")),
    }
}

fn run_game(name: &str, args: &[String]) -> Result<(), String>
{
    match name {
        "typing" => {
            let config = games::typing::TypingConfig::from_args(args)?;
            match openrgb::Keyboard::connect() {
                Ok(mut keyboard) => {
                    let device_name = keyboard.device_name().to_string();
                    games::typing::run_with_config(
                        Some(&mut keyboard),
                        &device_name,
                        config,
                    )?;
                }
                Err(err) => {
                    eprintln!(
                        "Warning: couldn't start RGB keyboard ({err}). Starting regular keyboard mode."
                    );
                    let device_name = "Regular keyboard".to_string();
                    games::typing::run_with_config(None, &device_name, config)?;
                }
            }
            Ok(())
        }
        "wordle" => {
            if !args.is_empty() {
                return Err("Wordle does not accept options yet.".to_string());
            }
            match openrgb::Keyboard::connect() {
                Ok(mut keyboard) => {
                    let device_name = keyboard.device_name().to_string();
                    games::wordle::run_with_keyboard(Some(&mut keyboard), &device_name)?;
                }
                Err(err) => {
                    eprintln!(
                        "Warning: couldn't start RGB keyboard ({err}). Starting regular keyboard mode."
                    );
                    let device_name = "Regular keyboard".to_string();
                    games::wordle::run_with_keyboard(None, &device_name)?;
                }
            }
            Ok(())
        }
        _ => Err(format!("Unknown game '{name}'. Run with --help.")),
    }
}

fn interactive_menu() -> Result<(), String>
{
    let registry = games::registry();
    println!("KB Games");
    println!();
    println!("Select a game:");
    for (idx, game) in registry.iter().enumerate() {
        println!("  {}. {} - {}", idx + 1, game.name, game.description);
    }
    println!();
    print!("Enter number or name (default 1, q to quit): ");
    std::io::Write::flush(&mut std::io::stdout())
        .map_err(|err| format!("Failed to flush stdout: {err}"))?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|err| format!("Failed to read input: {err}"))?;
    let choice = input.trim();

    if choice.is_empty() {
        return run_game(registry[0].name, &[]);
    }
    if choice.eq_ignore_ascii_case("q") {
        return Ok(());
    }
    if let Ok(index) = choice.parse::<usize>() {
        if index >= 1 && index <= registry.len() {
            return run_game(registry[index - 1].name, &[]);
        }
    }

    for game in registry {
        if game.name.eq_ignore_ascii_case(choice) {
            return run_game(game.name, &[]);
        }
    }

    Err("Invalid selection.".to_string())
}

fn list_games()
{
    println!("Available games:");
    for game in games::registry() {
        println!("  {:<10} - {}", game.name, game.description);
    }
}

fn print_help()
{
    println!("icue-kb-games");
    println!("\nUsage:");
    println!("  icue-kb-games list");
    println!("  icue-kb-games typing [--wpm=20]");
    println!("  icue-kb-games wordle");
    println!("\nNotes:");
    println!("  Start OpenRGB with the SDK server enabled (default 127.0.0.1:6742).");
    println!("  Set OPENRGB_HOST/OPENRGB_PORT to override the server location.");
}
