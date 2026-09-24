use whisperia_proto::control::Avatar;

pub fn animal_of(name: &str) -> Avatar {
    match name {
        "Panda" => Avatar::Panda,
        "Fox" => Avatar::Fox,
        "Mouse" => Avatar::Mouse,
        "Raccoon" => Avatar::Raccoon,
        "Octopus" => Avatar::Octopus,
        "Penguin" => Avatar::Penguin,
        _ => Avatar::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everybody_has_an_animal_of_their_own() {
        let people = ["Panda", "Fox", "Mouse", "Raccoon", "Octopus", "Penguin"];
        let mut animals: Vec<Avatar> = people.iter().map(|name| animal_of(name)).collect();
        assert!(!animals.contains(&Avatar::None));
        animals.sort_by_key(|animal| animal.key());
        animals.dedup();
        assert_eq!(animals.len(), people.len());
    }
}
