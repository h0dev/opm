pub trait Validatable {
    fn from_id(id: usize) -> Self;
    fn from_string(s: String) -> Self;
    fn get_string(&self) -> Option<&str>;
}

#[derive(Clone)]
pub enum Args {
    Id(usize),
    Script(String),
}

#[derive(Clone)]
pub enum Item {
    Id(usize),
    Name(String),
}

// Wrapper to support multiple items
#[derive(Clone)]
pub struct Items {
    pub items: Vec<Item>,
}

impl Items {
    pub fn single(item: Item) -> Self {
        Items { items: vec![item] }
    }

    pub fn multiple(items: Vec<Item>) -> Self {
        Items { items }
    }

    pub fn get_string(&self) -> Option<&str> {
        if self.items.len() == 1 {
            self.items[0].get_string()
        } else {
            None
        }
    }

    pub fn is_all(&self) -> bool {
        self.items.len() == 1 && self.items[0].get_string() == Some("all")
    }
}

impl Validatable for Args {
    fn from_id(id: usize) -> Self { Args::Id(id) }
    fn from_string(s: String) -> Self { Args::Script(s) }

    fn get_string(&self) -> Option<&str> {
        match self {
            Args::Id(_) => None,
            Args::Script(s) => Some(s),
        }
    }
}

impl Validatable for Item {
    fn from_id(id: usize) -> Self { Item::Id(id) }
    fn from_string(s: String) -> Self { Item::Name(s) }

    fn get_string(&self) -> Option<&str> {
        match self {
            Item::Id(_) => None,
            Item::Name(s) => Some(s),
        }
    }
}

pub fn validate<T: Validatable>(s: &str) -> Result<T, String> {
    if let Ok(id) = s.parse::<usize>() {
        Ok(T::from_id(id))
    } else {
        Ok(T::from_string(s.to_owned()))
    }
}

// Parse comma-separated items
pub fn validate_items(s: &str) -> Result<Items, String> {
    // First check if it's "all"
    if s.trim() == "all" {
        return Ok(Items::single(Item::Name("all".to_string())));
    }

    // Split by commas first, then trim whitespace
    let parts: Vec<&str> = s.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();

    if parts.is_empty() {
        return Err("No valid items provided".to_string());
    }

    let mut items = Vec::new();
    for part in parts {
        if let Ok(id) = part.parse::<usize>() {
            items.push(Item::Id(id));
        } else {
            items.push(Item::Name(part.to_owned()));
        }
    }

    Ok(Items::multiple(items))
}
