pub mod typing;
pub mod wordle;

pub struct GameDescriptor
{
    pub name: &'static str,
    pub description: &'static str,
}

pub fn registry() -> Vec<GameDescriptor>
{
    vec![GameDescriptor {
        name: "typing",
        description: "Fast typing with keyboard urgency colors",
    },
    GameDescriptor {
        name: "wordle",
        description: "Wordle-like with attempt review on the keyboard",
    }]
}
