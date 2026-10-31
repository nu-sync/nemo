//! What an out-of-tree engine needs from the machine before it can run.
//!
//! The included engine is bundled, so it needs nothing. An alternative engine that
//! runs as a helper needs its own toolchain, and that requirement outlives setup:
//! `uv` can disappear between two dictations. So this is checked before every load,
//! not only while installing, and a failure is a reason to fall back rather than a
//! reason to lose a dictation.

use std::process::Command;

/// The oldest Python the helper supports, matching its PEP 723 header.
pub const MINIMUM_PYTHON: (u32, u32) = (3, 12);

/// Everything the helper needs from the machine, as observed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    /// `uv --version` output, when the command ran.
    pub uv: Option<String>,
    /// `python3 --version` output, when the command ran.
    pub python: Option<String>,
}

impl Observed {
    /// Observe the machine. Cheap: two PATH lookups and two short subprocesses.
    pub fn current() -> Self {
        Self {
            uv: version_of("uv"),
            python: version_of("python3"),
        }
    }

    /// Classify the observation. Total, so a new state cannot be forgotten.
    pub fn state(&self) -> PrerequisiteState {
        if self.uv.is_none() {
            return PrerequisiteState::MissingUv;
        }
        match self.python.as_deref().and_then(parse_python) {
            None => PrerequisiteState::MissingPython,
            Some(found) if found < MINIMUM_PYTHON => PrerequisiteState::PythonTooOld {
                found: format!("{}.{}", found.0, found.1),
            },
            Some(_) => PrerequisiteState::Ready,
        }
    }
}

/// The prerequisite lifecycle of one alternative engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrerequisiteState {
    /// Every requirement is satisfied.
    Ready,
    /// `uv` is not on PATH.
    MissingUv,
    /// `python3` is not on PATH.
    MissingPython,
    /// Python is present but older than [`MINIMUM_PYTHON`].
    PythonTooOld { found: String },
}

impl PrerequisiteState {
    /// Whether the helper can start at all.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }

    /// A sentence a person can act on, or nothing when ready.
    pub fn reason(&self) -> Option<String> {
        match self {
            Self::Ready => None,
            Self::MissingUv => Some("uv is not on PATH".to_string()),
            Self::MissingPython => Some("python3 is not on PATH".to_string()),
            Self::PythonTooOld { found } => Some(format!(
                "python3 is {found}; this engine needs {}.{} or newer",
                MINIMUM_PYTHON.0, MINIMUM_PYTHON.1
            )),
        }
    }

    /// The exact command that fixes it. The app never runs this itself: installing
    /// system tooling needs admin rights and may be blocked by a managed machine.
    pub fn remedy(&self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::MissingUv => Some("brew install uv"),
            Self::MissingPython => Some("brew install python@3.12"),
            Self::PythonTooOld { .. } => Some("brew install python@3.12"),
        }
    }
}

fn version_of(program: &str) -> Option<String> {
    let output = Command::new(program).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `"Python 3.12.12"` -> `(3, 12)`.
fn parse_python(output: &str) -> Option<(u32, u32)> {
    let rest = output.trim().strip_prefix("Python ")?;
    let mut parts = rest.split('.');
    let major = parts.next()?.trim().parse().ok()?;
    let minor = parts.next()?.trim().parse().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(uv: Option<&str>, python: Option<&str>) -> Observed {
        Observed {
            uv: uv.map(str::to_string),
            python: python.map(str::to_string),
        }
    }

    #[test]
    fn a_machine_with_both_tools_is_ready() {
        let state = observed(Some("uv 0.13.4"), Some("Python 3.12.12")).state();
        assert_eq!(state, PrerequisiteState::Ready);
        assert!(state.is_ready());
        assert!(state.reason().is_none());
        assert!(state.remedy().is_none());
    }

    #[test]
    fn missing_uv_is_reported_first_because_it_is_what_launches_the_helper() {
        let state = observed(None, Some("Python 3.12.12")).state();
        assert_eq!(state, PrerequisiteState::MissingUv);
        assert!(!state.is_ready());
        assert_eq!(state.remedy(), Some("brew install uv"));
    }

    #[test]
    fn missing_python_and_an_old_python_are_distinct_remedies() {
        assert_eq!(
            observed(Some("uv 0.13.4"), None).state(),
            PrerequisiteState::MissingPython
        );
        let old = observed(Some("uv 0.13.4"), Some("Python 3.11.9")).state();
        assert_eq!(
            old,
            PrerequisiteState::PythonTooOld {
                found: "3.11".to_string()
            }
        );
        // the message names the requirement, not just the problem
        assert!(old.reason().unwrap_or_default().contains("3.12"));
    }

    #[test]
    fn version_parsing_accepts_the_shapes_python_prints() {
        assert_eq!(parse_python("Python 3.12.12"), Some((3, 12)));
        assert_eq!(parse_python("Python 3.13.0\n"), Some((3, 13)));
        assert_eq!(parse_python("graal 3.12"), None);
        assert_eq!(parse_python("Python"), None);
    }

    #[test]
    fn the_boundary_version_is_accepted_and_just_below_is_not() {
        assert!(
            observed(Some("uv"), Some("Python 3.12.0"))
                .state()
                .is_ready()
        );
        assert!(
            !observed(Some("uv"), Some("Python 3.11.99"))
                .state()
                .is_ready()
        );
    }
}
