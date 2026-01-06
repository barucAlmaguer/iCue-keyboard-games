pub mod typing;

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
    }]
}
