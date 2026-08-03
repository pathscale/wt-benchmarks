mod generator;
mod worktable_adapter;

use std::fmt::{Display, Formatter};
use std::str::FromStr;

pub use generator::{FIELD_COUNT, Operation, OperationKind, generate_streams, make_fields};
pub use worktable_adapter::run_repetition;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Workload {
    A,
    B,
    C,
    D,
    E,
    F,
}

impl Workload {
    pub fn default_distribution(self) -> Distribution {
        match self {
            Self::D => Distribution::Latest,
            Self::A | Self::B | Self::C | Self::E | Self::F => Distribution::Zipfian,
        }
    }
}

impl Display for Workload {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}",
            match self {
                Self::A => "A",
                Self::B => "B",
                Self::C => "C",
                Self::D => "D",
                Self::E => "E",
                Self::F => "F",
            }
        )
    }
}

impl FromStr for Workload {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_uppercase().as_str() {
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "C" => Ok(Self::C),
            "D" => Ok(Self::D),
            "E" => Ok(Self::E),
            "F" => Ok(Self::F),
            _ => Err(format!("unknown YCSB workload: {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Distribution {
    Uniform,
    Zipfian,
    Latest,
}

impl Display for Distribution {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}",
            match self {
                Self::Uniform => "uniform",
                Self::Zipfian => "zipfian",
                Self::Latest => "latest",
            }
        )
    }
}

impl FromStr for Distribution {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "uniform" => Ok(Self::Uniform),
            "zipf" | "zipfian" => Ok(Self::Zipfian),
            "latest" => Ok(Self::Latest),
            _ => Err(format!("unknown distribution: {value}")),
        }
    }
}
