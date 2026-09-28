use super::FixtureFileOutput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureSourcePairCommand {
    Help,
    Publish(FixtureFileOutput),
}

impl FixtureSourcePairCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Publish(_) => "publish",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(self, Self::Publish(_))
    }
}
