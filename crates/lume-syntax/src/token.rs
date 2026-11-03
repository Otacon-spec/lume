//! Classi semantiche dei token: il tema le mappa a colori altrove.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum TokenClass {
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Attribute,
    Constant,
    Operator,
    Punctuation,
    Variable,
    Property,
    Text,
}
