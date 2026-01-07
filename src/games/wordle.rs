use crate::openrgb::{Keyboard, LedColor};
use crate::words::WORDLE_WORDS;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::collections::HashMap;
use std::io::{self, Stdout, Write};
use std::time::{Duration, Instant};

const MIN_LEN: usize = 4;
const MAX_LEN: usize = 10;
const MAX_ATTEMPTS: usize = 6;
const TICK_MS: u64 = 33;
const BLINK_MS: u64 = 700;
const SEQ_STEP_MS: u128 = 220;
const SEQ_OFF_MS: u128 = 120;
const SEQ_PAUSE_MS: u128 = 2000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LetterState
{
    Correct,
    Present,
    Absent,
}

struct Attempt
{
    guess: String,
    states: Vec<LetterState>,
    is_win: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Rgb
{
    r: u8,
    g: u8,
    b: u8,
}

struct TerminalGuard
{
    stdout: Stdout,
}

impl TerminalGuard
{
    fn enter() -> io::Result<Self>
    {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, Hide)?;
        Ok(Self { stdout })
    }

    fn stdout(&mut self) -> &mut Stdout
    {
        &mut self.stdout
    }
}

impl Drop for TerminalGuard
{
    fn drop(&mut self)
    {
        let _ = execute!(self.stdout, Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

pub fn run_with_keyboard(
    mut keyboard: Option<&mut Keyboard>,
    device_name: &str,
) -> Result<(), String>
{
    let mut term = TerminalGuard::enter().map_err(|err| err.to_string())?;
    let mut rng = thread_rng();
    let secret = WORDLE_WORDS
        .choose(&mut rng)
        .ok_or_else(|| "Word list is empty".to_string())?
        .to_string();

    let mut attempts: Vec<Attempt> = Vec::new();
    let mut current_guess = String::new();
    let mut selected_attempt: usize = 0;
    let mut message: Option<String> = None;

    let start = Instant::now();
    let mut last_tick = Instant::now();

    loop {
        let current_attempt = attempts.len();
        if selected_attempt > current_attempt {
            selected_attempt = current_attempt;
        }

        if is_game_over(&attempts) {
            break;
        }

        if handle_input(
            &mut current_guess,
            &mut attempts,
            &secret,
            &mut selected_attempt,
            &mut message,
        )? {
            break;
        }

        if last_tick.elapsed() >= Duration::from_millis(TICK_MS) {
            let blink_on = (start.elapsed().as_millis() / BLINK_MS as u128) % 2 == 0;
            if let Some(kbd) = keyboard.as_deref_mut() {
                let leds = build_keyboard_leds(
                    kbd,
                    &attempts,
                    &current_guess,
                    selected_attempt,
                    blink_on,
                    start,
                )?;
                kbd.set_leds(&leds)?;
            }

            draw_ui(
                term.stdout(),
                device_name,
                &attempts,
                &current_guess,
                selected_attempt,
                &message,
            )?;
            last_tick = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(1));
    }

    draw_summary(term.stdout(), device_name, &secret, &attempts)?;
    if let Some(kbd) = keyboard.as_deref_mut() {
        set_finish_leds(kbd)?;
    }
    wait_for_space()?;
    Ok(())
}

fn handle_input(
    current_guess: &mut String,
    attempts: &mut Vec<Attempt>,
    secret: &str,
    selected_attempt: &mut usize,
    message: &mut Option<String>,
) -> Result<bool, String>
{
    while event::poll(Duration::from_millis(0)).map_err(|err| err.to_string())? {
        match event::read().map_err(|err| err.to_string())? {
            Event::Key(KeyEvent { code, modifiers, .. }) => match code {
                KeyCode::Esc => return Ok(true),
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(true)
                }
                KeyCode::Left => {
                    if *selected_attempt > 0 {
                        *selected_attempt -= 1;
                    }
                }
                KeyCode::Right => {
                    if *selected_attempt < attempts.len() {
                        *selected_attempt += 1;
                    }
                }
                KeyCode::Backspace => {
                    if *selected_attempt == attempts.len() {
                        current_guess.pop();
                    }
                }
                KeyCode::Enter => {
                    if *selected_attempt != attempts.len() {
                        continue;
                    }
                    if current_guess.len() < MIN_LEN || current_guess.len() > MAX_LEN {
                        *message = Some(format!(
                            "Guess length must be {}-{} letters",
                            MIN_LEN, MAX_LEN
                        ));
                        continue;
                    }
                    if attempts.len() >= MAX_ATTEMPTS {
                        continue;
                    }

                    let states = evaluate_guess(secret, current_guess);
                    let is_win = current_guess == secret;
                    attempts.push(Attempt {
                        guess: current_guess.clone(),
                        states,
                        is_win,
                    });
                    current_guess.clear();
                    *message = None;
                    *selected_attempt = attempts.len();
                }
                KeyCode::Char(ch) => {
                    if *selected_attempt != attempts.len() {
                        continue;
                    }
                    if ch.is_ascii_alphabetic() && current_guess.len() < MAX_LEN {
                        current_guess.push(ch.to_ascii_lowercase());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    Ok(false)
}

fn is_game_over(attempts: &[Attempt]) -> bool
{
    attempts
        .last()
        .is_some_and(|attempt| attempt.is_win)
        || attempts.len() >= MAX_ATTEMPTS
}

fn evaluate_guess(secret: &str, guess: &str) -> Vec<LetterState>
{
    let secret_chars: Vec<char> = secret.chars().collect();
    let guess_chars: Vec<char> = guess.chars().collect();
    let mut states = vec![LetterState::Absent; guess_chars.len()];

    let mut remaining: HashMap<char, usize> = HashMap::new();
    let min_len = secret_chars.len().min(guess_chars.len());

    for i in 0..min_len {
        if guess_chars[i] == secret_chars[i] {
            states[i] = LetterState::Correct;
        } else {
            *remaining.entry(secret_chars[i]).or_insert(0) += 1;
        }
    }

    for i in min_len..secret_chars.len() {
        *remaining.entry(secret_chars[i]).or_insert(0) += 1;
    }

    for i in 0..guess_chars.len() {
        if states[i] == LetterState::Correct {
            continue;
        }
        let ch = guess_chars[i];
        if let Some(count) = remaining.get_mut(&ch) {
            if *count > 0 {
                states[i] = LetterState::Present;
                *count -= 1;
            }
        }
    }

    states
}

fn attempt_status_color(attempt: &Attempt, start: Instant) -> Rgb
{
    if attempt.is_win {
        return Rgb { r: 0, g: 255, b: 0 };
    }

    let mut greens = 0usize;
    let mut yellows = 0usize;
    let mut reds = 0usize;
    for state in &attempt.states {
        match state {
            LetterState::Correct => greens += 1,
            LetterState::Present => yellows += 1,
            LetterState::Absent => reds += 1,
        }
    }

    if greens == 0 {
        if yellows > 0 {
            return Rgb { r: 255, g: 140, b: 0 };
        }
        return Rgb { r: 255, g: 0, b: 0 };
    }

    let total = (greens + yellows + reds).max(1) as u128;
    let cycle = (BLINK_MS as u128) * 3;
    let pos = start.elapsed().as_millis() % cycle;
    let green_window = (cycle * greens as u128) / total;
    let yellow_window = (cycle * yellows as u128) / total;

    if pos < green_window {
        Rgb { r: 0, g: 255, b: 0 }
    } else if pos < green_window + yellow_window {
        Rgb { r: 255, g: 215, b: 0 }
    } else {
        Rgb { r: 0, g: 0, b: 0 }
    }
}

fn build_keyboard_leds(
    keyboard: &Keyboard,
    attempts: &[Attempt],
    current_guess: &str,
    selected_attempt: usize,
    blink_on: bool,
    start: Instant,
) -> Result<Vec<LedColor>, String>
{
    let mut map: HashMap<u32, Rgb> = HashMap::new();
    let current_attempt = attempts.len();

    for attempt_idx in 0..MAX_ATTEMPTS {
        let key_char = attempt_key_char(attempt_idx);
        if let Some(id) = keyboard.led_for_char(key_char) {
            let color = if attempt_idx < attempts.len() {
                attempt_status_color(&attempts[attempt_idx], start)
            } else {
                Rgb { r: 0, g: 0, b: 0 }
            };
            map.insert(id, color);
        }
    }

    if current_attempt < MAX_ATTEMPTS {
        let key_char = attempt_key_char(current_attempt);
        if let Some(id) = keyboard.led_for_char(key_char) {
            if blink_on {
                map.insert(id, Rgb { r: 255, g: 255, b: 255 });
            } else if current_attempt < attempts.len() {
                map.insert(id, attempt_status_color(&attempts[current_attempt], start));
            } else {
                map.remove(&id);
            }
        }
    }

    if selected_attempt < attempts.len() && selected_attempt != current_attempt {
        let key_char = attempt_key_char(selected_attempt);
        if let Some(id) = keyboard.led_for_char(key_char) {
            map.insert(id, Rgb { r: 255, g: 255, b: 255 });
        }
    }

    if selected_attempt < attempts.len() {
        apply_attempt_colors(&mut map, keyboard, &attempts[selected_attempt]);
    } else {
        apply_letter_baseline(&mut map, keyboard);
        if let Some(last_attempt) = attempts.last() {
            apply_attempt_colors(&mut map, keyboard, last_attempt);
        }
        apply_current_guess(&mut map, keyboard, current_guess);
    }

    let blink_word = if selected_attempt < attempts.len() {
        Some(attempts[selected_attempt].guess.as_str())
    } else if !current_guess.is_empty() {
        Some(current_guess)
    } else {
        attempts.last().map(|attempt| attempt.guess.as_str())
    };

    if let Some(word) = blink_word {
        if let Some(ch) = blink_sequence_char(word, start) {
            if let Some(id) = keyboard.led_for_char(ch) {
                map.insert(id, Rgb { r: 0, g: 0, b: 0 });
            }
        }
    }

    let leds = map
        .into_iter()
        .map(|(id, color)| LedColor {
            id,
            r: color.r,
            g: color.g,
            b: color.b,
        })
        .collect();

    Ok(leds)
}

fn attempt_key_char(index: usize) -> char
{
    match index {
        0 => '1',
        1 => '2',
        2 => '3',
        3 => '4',
        4 => '5',
        5 => '6',
        _ => '0',
    }
}

fn apply_attempt_colors(map: &mut HashMap<u32, Rgb>, keyboard: &Keyboard, attempt: &Attempt)
{
    for (ch, state) in attempt.guess.chars().zip(attempt.states.iter()) {
        if let Some(id) = keyboard.led_for_char(ch) {
            let color = match state {
                LetterState::Correct => Rgb { r: 0, g: 255, b: 0 },
                LetterState::Present => Rgb { r: 255, g: 215, b: 0 },
                LetterState::Absent => Rgb { r: 255, g: 0, b: 0 },
            };
            let entry = map.entry(id).or_insert(color);
            if priority(color) > priority(*entry) {
                *entry = color;
            }
        }
    }
}

fn apply_current_guess(map: &mut HashMap<u32, Rgb>, keyboard: &Keyboard, guess: &str)
{
    for ch in guess.chars() {
        if let Some(id) = keyboard.led_for_char(ch) {
            map.insert(id, Rgb { r: 80, g: 140, b: 255 });
        }
    }
}

fn blink_sequence_char(word: &str, start: Instant) -> Option<char>
{
    let letters: Vec<char> = word.chars().collect();
    if letters.is_empty() {
        return None;
    }
    let seq_len = SEQ_STEP_MS * (letters.len() as u128);
    let cycle = seq_len + SEQ_PAUSE_MS;
    let elapsed = start.elapsed().as_millis() % cycle;
    if elapsed >= seq_len {
        return None;
    }
    let idx = (elapsed / SEQ_STEP_MS) as usize;
    let step_pos = elapsed % SEQ_STEP_MS;
    if step_pos < SEQ_OFF_MS {
        letters.get(idx).copied()
    } else {
        None
    }
}

fn apply_letter_baseline(map: &mut HashMap<u32, Rgb>, keyboard: &Keyboard)
{
    for ch in 'a'..='z' {
        if let Some(id) = keyboard.led_for_char(ch) {
            map.entry(id).or_insert(Rgb { r: 255, g: 255, b: 255 });
        }
    }
}

fn priority(color: Rgb) -> u8
{
    if color == (Rgb { r: 0, g: 255, b: 0 }) {
        3
    } else if color == (Rgb { r: 255, g: 215, b: 0 }) {
        2
    } else if color == (Rgb { r: 255, g: 0, b: 0 }) {
        1
    } else {
        0
    }
}

fn draw_ui(
    stdout: &mut Stdout,
    device_name: &str,
    attempts: &[Attempt],
    current_guess: &str,
    selected_attempt: usize,
    message: &Option<String>,
) -> Result<(), String>
{
    let mut lines = Vec::new();
    lines.push("KB Games - Wordle".to_string());
    lines.push(format!("Keyboard: {}", device_name));
    lines.push(format!(
        "Attempt {}/{}  Guess length: {}-{}",
        attempts.len() + 1,
        MAX_ATTEMPTS,
        MIN_LEN,
        MAX_LEN
    ));
    lines.push(String::new());

    for (idx, attempt) in attempts.iter().enumerate() {
        let mut row = render_attempt(attempt);
        if idx == selected_attempt {
            row.push_str("  <");
        }
        lines.push(row);
    }

    if attempts.len() < MAX_ATTEMPTS {
        let mut row = render_current_guess(current_guess);
        if selected_attempt == attempts.len() {
            row.push_str("  <");
        }
        lines.push(row);
    }

    lines.push(String::new());
    if let Some(msg) = message {
        lines.push(format!("{}", msg));
    } else {
        lines.push("Use Left/Right to review attempts. Enter to submit.".to_string());
    }
    lines.push("Backspace edits. Esc quits.".to_string());

    let output = format!("{}\r\n", lines.join("\r\n"));
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))
        .map_err(|err| err.to_string())?;
    stdout.write_all(output.as_bytes()).map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;
    Ok(())
}

fn render_attempt(attempt: &Attempt) -> String
{
    let mut row = String::new();
    for (ch, state) in attempt.guess.chars().zip(attempt.states.iter()) {
        let (r, g, b) = match state {
            LetterState::Correct => (0, 150, 70),
            LetterState::Present => (180, 130, 0),
            LetterState::Absent => (90, 20, 20),
        };
        row.push_str(&format!("\x1b[48;2;{};{};{}m {} \x1b[0m", r, g, b, ch.to_ascii_uppercase()));
    }
    row
}

fn render_current_guess(guess: &str) -> String
{
    let mut row = String::new();
    if guess.is_empty() {
        row.push_str("(type a guess)");
    } else {
        for ch in guess.chars() {
            row.push_str(&format!("\x1b[48;2;40;40;40m {} \x1b[0m", ch.to_ascii_uppercase()));
        }
    }
    row
}

fn draw_summary(
    stdout: &mut Stdout,
    device_name: &str,
    secret: &str,
    attempts: &[Attempt],
) -> Result<(), String>
{
    let win = attempts.last().is_some_and(|attempt| attempt.is_win);
    let mut lines = Vec::new();
    lines.push("Game over".to_string());
    lines.push(String::new());
    lines.push(format!("Keyboard: {}", device_name));
    lines.push(format!("Secret word: {}", secret.to_ascii_uppercase()));
    lines.push(format!(
        "Result: {}",
        if win { "Solved" } else { "Out of attempts" }
    ));
    lines.push(String::new());
    lines.push("Press SPACE to exit.".to_string());

    let output = format!("{}\r\n", lines.join("\r\n"));
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))
        .map_err(|err| err.to_string())?;
    stdout.write_all(output.as_bytes()).map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;
    Ok(())
}

fn wait_for_space() -> Result<(), String>
{
    while event::poll(Duration::from_millis(0)).map_err(|err| err.to_string())? {
        let _ = event::read().map_err(|err| err.to_string())?;
    }

    loop {
        if event::poll(Duration::from_millis(50)).map_err(|err| err.to_string())? {
            if let Event::Key(KeyEvent { code: KeyCode::Char(' '), .. }) =
                event::read().map_err(|err| err.to_string())?
            {
                break;
            }
        }
    }

    Ok(())
}

fn set_finish_leds(keyboard: &mut Keyboard) -> Result<(), String>
{
    let glow = Rgb { r: 255, g: 215, b: 0 };
    if let Some(id) = keyboard.led_for_char(' ') {
        keyboard.set_leds(&[LedColor {
            id,
            r: glow.r,
            g: glow.g,
            b: glow.b,
        }])?;
    }
    Ok(())
}
