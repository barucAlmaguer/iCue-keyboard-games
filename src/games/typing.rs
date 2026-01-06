use crate::openrgb::{Keyboard, LedColor};
use crate::words::{BONUS_WORDS, WORDS};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, queue};
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::HashMap;
use std::io::{self, Stdout, Write};
use std::time::{Duration, Instant};

const LEVEL_DURATION: Duration = Duration::from_secs(60);
const START_LIVES: u8 = 5;
const MAX_WORDS: usize = 5;
const TICK_MS: u64 = 33;
const SPAWN_INTERVAL: Duration = Duration::from_millis(1400);
const BONUS_INTERVAL: u32 = 10;
const DEFAULT_WPM: f32 = 20.0;
const MIN_WPM: f32 = 5.0;
const MAX_WPM: f32 = 120.0;

#[derive(Clone)]
struct Word
{
    text: String,
    spawned_at: Instant,
    ttl: Duration,
    column: usize,
    color: Option<Rgb>,
    is_bonus: bool,
}

#[derive(Default)]
struct Stats
{
    words_typed: u32,
    words_missed: u32,
    keystrokes: u32,
    backspaces: u32,
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct Rgb
{
    r: u8,
    g: u8,
    b: u8,
}

#[derive(Clone, Copy)]
struct Cell
{
    ch: char,
    color: Option<Rgb>,
}

pub struct TypingConfig
{
    start_wpm: f32,
    speed_scale: f32,
}

impl TypingConfig
{
    pub fn from_args(args: &[String]) -> Result<Self, String>
    {
        let mut wpm: Option<f32> = None;
        let mut iter = args.iter().peekable();
        while let Some(arg) = iter.next() {
            if arg == "--wpm" {
                let value = iter
                    .next()
                    .ok_or_else(|| "Expected value after --wpm".to_string())?;
                wpm = Some(parse_wpm(value)?);
            } else if let Some(rest) = arg.strip_prefix("--wpm=") {
                wpm = Some(parse_wpm(rest)?);
            } else {
                return Err(format!("Unknown typing option '{arg}'"));
            }
        }

        let start_wpm = wpm.unwrap_or(DEFAULT_WPM);
        Ok(Self::new(start_wpm))
    }

    fn new(start_wpm: f32) -> Self
    {
        let clamped = start_wpm.clamp(MIN_WPM, MAX_WPM);
        let scale = (DEFAULT_WPM / clamped).clamp(0.4, 2.5);
        Self {
            start_wpm: clamped,
            speed_scale: scale,
        }
    }
}

impl Default for TypingConfig
{
    fn default() -> Self
    {
        Self::new(DEFAULT_WPM)
    }
}

fn parse_wpm(value: &str) -> Result<f32, String>
{
    let parsed = value
        .parse::<f32>()
        .map_err(|_| "WPM must be a number".to_string())?;
    if parsed <= 0.0 {
        return Err("WPM must be positive".to_string());
    }
    Ok(parsed)
}

pub fn run_with_config(keyboard: &mut Keyboard, config: TypingConfig) -> Result<(), String>
{
    let mut term = TerminalGuard::enter().map_err(|err| err.to_string())?;
    let mut rng = rand::thread_rng();

    let start = Instant::now();
    let mut next_spawn = start;
    let mut words: Vec<Word> = Vec::new();
    let mut buffer = String::new();
    let mut stats = Stats::default();
    let mut lives = START_LIVES;
    let mut last_tick = Instant::now();
    let mut bonus_ready = false;
    let mut words_since_bonus = 0u32;
    let spawn_interval = scaled_duration(SPAWN_INTERVAL, config.speed_scale);

    loop {
        let now = Instant::now();
        let (field_width, field_height) = layout_metrics();
        let elapsed = now.saturating_duration_since(start);
        if elapsed >= LEVEL_DURATION || lives == 0 {
            break;
        }

        if handle_input(&mut buffer, &mut stats)? {
            break;
        }

        if words.is_empty() {
            let word = spawn_word(&mut rng, now, elapsed, field_width, bonus_ready, &config);
            if bonus_ready {
                bonus_ready = false;
            }
            words.push(word);
            next_spawn = now + spawn_interval;
        } else if now >= next_spawn {
            if words.len() < MAX_WORDS {
                let word = spawn_word(&mut rng, now, elapsed, field_width, bonus_ready, &config);
                if bonus_ready {
                    bonus_ready = false;
                }
                words.push(word);
            }
            next_spawn = now + spawn_interval;
        }

        let before = words.len();
        words.retain(|word| now.saturating_duration_since(word.spawned_at) < word.ttl);
        let expired = before - words.len();
        if expired > 0 {
            let lost = expired.min(lives as usize) as u8;
            lives = lives.saturating_sub(lost);
            stats.words_missed += expired as u32;
        }

        if !buffer.is_empty() {
            if let Some(index) = words.iter().position(|word| word.text == buffer) {
                let word = words.swap_remove(index);
                stats.words_typed += 1;
                if word.is_bonus {
                    lives = (lives + 1).min(START_LIVES);
                } else {
                    words_since_bonus += 1;
                    if words_since_bonus >= BONUS_INTERVAL {
                        bonus_ready = true;
                        words_since_bonus = 0;
                    }
                }
                buffer.clear();
            }
        }

        if last_tick.elapsed() >= Duration::from_millis(TICK_MS) {
            let leds = build_leds(keyboard, &words, lives, now)?;
            keyboard.set_leds(&leds)?;

            draw_ui(
                term.stdout(),
                keyboard.device_name(),
                &words,
                &buffer,
                &stats,
                lives,
                elapsed,
                now,
                field_width,
                field_height,
                config.start_wpm,
            )?;

            last_tick = Instant::now();
        }

        std::thread::sleep(Duration::from_millis(1));
    }

    draw_summary(
        term.stdout(),
        keyboard.device_name(),
        &stats,
        start.elapsed().min(LEVEL_DURATION),
        lives,
    )?;
    set_finish_leds(keyboard, lives)?;
    wait_for_exit()?;
    Ok(())
}

fn handle_input(buffer: &mut String, stats: &mut Stats) -> Result<bool, String>
{
    while event::poll(Duration::from_millis(0)).map_err(|err| err.to_string())? {
        match event::read().map_err(|err| err.to_string())? {
            Event::Key(KeyEvent { code, modifiers, .. }) => match code {
                KeyCode::Esc => return Ok(true),
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(true)
                }
                KeyCode::Backspace => {
                    stats.backspaces += 1;
                    buffer.pop();
                }
                KeyCode::Enter => {
                    buffer.clear();
                }
                KeyCode::Char(ch) => {
                    if ch.is_ascii_alphabetic() {
                        stats.keystrokes += 1;
                        buffer.push(ch.to_ascii_lowercase());
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    Ok(false)
}

fn spawn_word(
    rng: &mut impl Rng,
    now: Instant,
    elapsed: Duration,
    field_width: usize,
    is_bonus: bool,
    config: &TypingConfig,
) -> Word
{
    let ttl = word_ttl(rng, elapsed, config);
    let word = if is_bonus {
        BONUS_WORDS.choose(rng).unwrap_or(&"constellation")
    } else {
        WORDS.choose(rng).unwrap_or(&"alpha")
    };
    let color = if is_bonus {
        Some(Rgb {
            r: 255,
            g: 215,
            b: 0,
        })
    } else {
        None
    };
    let word_len = word.len();
    let max_col = field_width.saturating_sub(word_len);
    let column = if max_col == 0 {
        0
    } else {
        rng.gen_range(0..=max_col)
    };
    Word {
        text: word.to_string(),
        spawned_at: now,
        ttl,
        column,
        color,
        is_bonus,
    }
}

fn word_ttl(rng: &mut impl Rng, elapsed: Duration, config: &TypingConfig) -> Duration
{
    let progress = (elapsed.as_secs_f32() / LEVEL_DURATION.as_secs_f32()).clamp(0.0, 1.0);
    let base = lerp(5.0, 2.0, progress);
    let jitter = rng.gen_range(0.75..1.25);
    let scaled = (base * config.speed_scale).clamp(0.8, 8.0);
    Duration::from_millis((scaled * jitter * 1000.0) as u64)
}

fn draw_ui(
    stdout: &mut Stdout,
    device_model: &str,
    words: &[Word],
    buffer: &str,
    stats: &Stats,
    lives: u8,
    elapsed: Duration,
    now: Instant,
    field_width: usize,
    field_height: usize,
    start_wpm: f32,
) -> Result<(), String>
{
    let time_left = (LEVEL_DURATION.as_secs_f32() - elapsed.as_secs_f32()).max(0.0);
    let mut lines = Vec::new();
    lines.push("KB Games - Fast Typing".to_string());
    lines.push(format!("Keyboard: {}", device_model));
    lines.push(format!(
        "Time left: {:>5.1}s  Lives: {}/{}  On screen: {}  Start WPM: {:>4.0}",
        time_left,
        lives,
        START_LIVES,
        words.len(),
        start_wpm
    ));
    lines.push(format!(
        "Typed: {}  Missed: {}  WPM: {:>5.1}",
        stats.words_typed,
        stats.words_missed,
        compute_wpm(stats.words_typed, elapsed)
    ));
    let field_width = field_width.max(1);
    let field_height = field_height.max(1);
    let mut field = vec![
        vec![
            Cell {
                ch: ' ',
                color: None,
            };
            field_width
        ];
        field_height
    ];
    let buffer_len = buffer.chars().count();
    for word in words {
        let age = now.saturating_duration_since(word.spawned_at);
        let progress = if word.ttl.as_secs_f32() <= 0.0 {
            1.0
        } else {
            (age.as_secs_f32() / word.ttl.as_secs_f32()).clamp(0.0, 1.0)
        };
        let row = ((field_height as f32 - 1.0) * progress).floor() as usize;
        let col = word.column.min(field_width.saturating_sub(1));
        let max_len = field_width.saturating_sub(col);
        let text = if word.text.len() > max_len {
            &word.text[..max_len]
        } else {
            word.text.as_str()
        };
        let prefix_match = buffer_len > 0 && word.text.starts_with(buffer);
        for (offset, ch) in text.chars().enumerate() {
            if col + offset < field_width && row < field_height {
                let cell_color = if prefix_match && offset < buffer_len {
                    Some(Rgb { r: 0, g: 255, b: 0 })
                } else {
                    word.color
                };
                field[row][col + offset] = Cell {
                    ch,
                    color: cell_color,
                };
            }
        }
    }

    for row in field {
        lines.push(render_row(&row));
    }
    lines.push("=".repeat(field_width));

    lines.push(format!("Input: {}", buffer));
    lines.push(format!(
        "Status: {}",
        if buffer.is_empty() {
            "waiting"
        } else if matches_prefix(buffer, words) {
            "ok"
        } else {
            "no match"
        }
    ));
    lines.push("Controls: type words, backspace/enter to clear, ESC to quit".to_string());

    let output = format!("{}\r\n", lines.join("\r\n"));

    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))
        .map_err(|err| err.to_string())?;
    stdout.write_all(output.as_bytes()).map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;

    Ok(())
}

fn draw_summary(
    stdout: &mut Stdout,
    device_model: &str,
    stats: &Stats,
    elapsed: Duration,
    lives: u8,
) -> Result<(), String>
{
    let mut lines = Vec::new();
    lines.push("Level complete".to_string());
    lines.push(String::new());
    lines.push(format!("Keyboard: {}", device_model));
    lines.push(format!("Duration: {:>5.1}s", elapsed.as_secs_f32()));
    lines.push(format!("Lives left: {}", lives));
    lines.push(format!("Words typed: {}", stats.words_typed));
    lines.push(format!("Words missed: {}", stats.words_missed));
    lines.push(format!("WPM: {:>5.1}", compute_wpm(stats.words_typed, elapsed)));
    lines.push(format!(
        "Accuracy: {:>5.1}%",
        compute_accuracy(stats.words_typed, stats.words_missed)
    ));
    lines.push(format!("Keystrokes: {}", stats.keystrokes));
    lines.push(format!("Backspaces: {}", stats.backspaces));
    lines.push(String::new());
    lines.push("Press SPACE to exit.".to_string());

    let output = format!("{}\r\n", lines.join("\r\n"));

    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))
        .map_err(|err| err.to_string())?;
    stdout.write_all(output.as_bytes()).map_err(|err| err.to_string())?;
    stdout.flush().map_err(|err| err.to_string())?;

    Ok(())
}

fn wait_for_exit() -> Result<(), String>
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

fn matches_prefix(buffer: &str, words: &[Word]) -> bool
{
    words.iter().any(|word| word.text.starts_with(buffer))
}

fn compute_wpm(words_typed: u32, elapsed: Duration) -> f32
{
    let minutes = elapsed.as_secs_f32() / 60.0;
    if minutes <= 0.0 {
        return 0.0;
    }
    words_typed as f32 / minutes
}

fn compute_accuracy(words_typed: u32, words_missed: u32) -> f32
{
    let total = words_typed + words_missed;
    if total == 0 {
        return 0.0;
    }
    (words_typed as f32 / total as f32) * 100.0
}

fn build_leds(
    keyboard: &Keyboard,
    words: &[Word],
    lives: u8,
    now: Instant,
) -> Result<Vec<LedColor>, String>
{
    let mut map: HashMap<u32, (Rgb, f32)> = HashMap::new();

    for word in words {
        let age = now.saturating_duration_since(word.spawned_at);
        let urgency = if word.ttl.as_secs_f32() == 0.0 {
            1.0
        } else {
            (age.as_secs_f32() / word.ttl.as_secs_f32()).clamp(0.0, 1.0)
        };
        let color = color_for_urgency(urgency);

        for ch in word.text.chars() {
            if let Some(id) = keyboard.led_for_char(ch) {
                let entry = map.entry(id).or_insert((color, urgency));
                if urgency > entry.1 {
                    *entry = (color, urgency);
                }
            }
        }
    }

    let red = Rgb { r: 255, g: 0, b: 0 };
    let off = Rgb { r: 0, g: 0, b: 0 };
    for i in 1..=START_LIVES {
        if let Some(id) = keyboard.led_for_char(char::from_digit(i as u32, 10).unwrap()) {
            let color = if i <= lives { red } else { off };
            map.insert(id, (color, 2.0));
        }
    }

    let leds = map
        .into_iter()
        .map(|(id, (color, _))| LedColor {
            id,
            r: color.r,
            g: color.g,
            b: color.b,
        })
        .collect();

    Ok(leds)
}

fn set_finish_leds(keyboard: &mut Keyboard, lives: u8) -> Result<(), String>
{
    let mut leds = Vec::new();
    let red = Rgb { r: 255, g: 0, b: 0 };
    let off = Rgb { r: 0, g: 0, b: 0 };
    let glow = Rgb { r: 255, g: 215, b: 0 };

    for i in 1..=START_LIVES {
        if let Some(id) = keyboard.led_for_char(char::from_digit(i as u32, 10).unwrap()) {
            let color = if i <= lives { red } else { off };
            leds.push(LedColor {
                id,
                r: color.r,
                g: color.g,
                b: color.b,
            });
        }
    }

    if let Some(id) = keyboard.led_for_char(' ') {
        leds.push(LedColor {
            id,
            r: glow.r,
            g: glow.g,
            b: glow.b,
        });
    }

    keyboard.set_leds(&leds)?;
    Ok(())
}

fn color_for_urgency(progress: f32) -> Rgb
{
    let progress = progress.clamp(0.0, 1.0);
    let green = Rgb { r: 0, g: 255, b: 0 };
    let yellow = Rgb { r: 255, g: 255, b: 0 };
    let orange = Rgb { r: 255, g: 128, b: 0 };
    let red = Rgb { r: 255, g: 0, b: 0 };

    if progress < 0.33 {
        lerp_color(green, yellow, progress / 0.33)
    } else if progress < 0.66 {
        lerp_color(yellow, orange, (progress - 0.33) / 0.33)
    } else {
        lerp_color(orange, red, (progress - 0.66) / 0.34)
    }
}

fn lerp_color(start: Rgb, end: Rgb, t: f32) -> Rgb
{
    let t = t.clamp(0.0, 1.0);
    Rgb {
        r: lerp(start.r as f32, end.r as f32, t) as u8,
        g: lerp(start.g as f32, end.g as f32, t) as u8,
        b: lerp(start.b as f32, end.b as f32, t) as u8,
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32
{
    a + (b - a) * t
}

fn layout_metrics() -> (usize, usize)
{
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let width = cols as usize;
    let height = rows as usize;
    let header_lines = 4;
    let footer_lines = 3;
    let extra = header_lines + 1 + footer_lines;
    let mut field_height = if height > extra { height - extra } else { 6 };
    field_height = field_height.clamp(8, 22);
    let mut field_width = width.saturating_sub(2).max(10);
    if field_width > width && width > 0 {
        field_width = width;
    }
    (field_width, field_height)
}

fn scaled_duration(base: Duration, scale: f32) -> Duration
{
    let millis = base.as_secs_f32() * 1000.0 * scale;
    Duration::from_millis(millis.max(100.0) as u64)
}

fn render_row(row: &[Cell]) -> String
{
    let mut line = String::with_capacity(row.len() + 16);
    let mut active: Option<Rgb> = None;
    for cell in row {
        if cell.color != active {
            if let Some(color) = cell.color {
                line.push_str(&ansi_color(color));
            } else {
                line.push_str("\x1b[0m");
            }
            active = cell.color;
        }
        line.push(cell.ch);
    }
    if active.is_some() {
        line.push_str("\x1b[0m");
    }
    line
}

fn ansi_color(color: Rgb) -> String
{
    format!("\x1b[38;2;{};{};{}m", color.r, color.g, color.b)
}
