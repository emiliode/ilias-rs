use snafu::{whatever, ResultExt, Whatever};

use crate::{client::IliasClient, IliasElement};

#[derive(Debug)]
pub enum Reference<T> {
    Unavailable,
    Unresolved(String),
    Resolved(T),
}

impl<T> Reference<T> {
    pub fn from_optional_querypath(querypath: Option<String>) -> Reference<T> {
        match querypath {
            None => Self::Unavailable,
            Some(querypath) => Self::Unresolved(querypath),
        }
    }

    pub fn try_get_resolved(&self) -> Option<&T> {
        match self {
            Self::Resolved(t) => Some(t),
            _ => None,
        }
    }
}

impl<T: IliasElement> Reference<T> {
    pub fn resolve(&self, ilias_client: &IliasClient) -> Result<T, Whatever> {
        let querypath = match self {
            Self::Unavailable => whatever!("Reference unavailable"),
            Self::Resolved(_) => whatever!("Already resolved"),
            Self::Unresolved(querypath) => querypath,
        };

        let element = ilias_client
            .get_querypath(querypath)
            .whatever_context("Could not get querypath from element")?;
        T::parse(element.root_element(), ilias_client)
    }
}
