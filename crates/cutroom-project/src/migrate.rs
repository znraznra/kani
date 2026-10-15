use crate::doc::SCHEMA_VERSION;
use crate::error::ProjectError;
use serde_json::Value;

type Step = fn(Value) -> Result<Value, ProjectError>;

/// `STEPS[n]` upgrades schema version `n + 1` to `n + 2`.
/// Empty while v1 is the newest format.
const STEPS: &[Step] = &[];

/// Upgrade a project file's JSON to the current schema.
pub fn migrate(value: Value) -> Result<Value, ProjectError> {
    migrate_with(value, SCHEMA_VERSION, STEPS)
}

pub(crate) fn migrate_with(
    mut value: Value,
    target: u32,
    steps: &[Step],
) -> Result<Value, ProjectError> {
    let found = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| ProjectError::Corrupt("missing schema_version".into()))?;
    let mut version = u32::try_from(found)
        .map_err(|_| ProjectError::Corrupt("schema_version out of range".into()))?;
    if version == 0 {
        return Err(ProjectError::Corrupt(
            "schema_version must be at least 1".into(),
        ));
    }
    if version > target {
        return Err(ProjectError::TooNew {
            found: version,
            supported: target,
        });
    }
    while version < target {
        let step = steps
            .get(version as usize - 1)
            .ok_or_else(|| ProjectError::Corrupt(format!("no migration from schema {version}")))?;
        value = step(value)?;
        version += 1;
        value["schema_version"] = Value::from(version);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v1_to_v2(mut value: Value) -> Result<Value, ProjectError> {
        // Pretend v2 renamed "title" to "name".
        let title = value["project"]["title"].take();
        value["project"]["name"] = title;
        Ok(value)
    }

    #[test]
    fn runs_steps_in_order() {
        let old = json!({"schema_version": 1, "project": {"title": "demo"}});
        let new = migrate_with(old, 2, &[v1_to_v2]).unwrap();
        assert_eq!(new["schema_version"], 2);
        assert_eq!(new["project"]["name"], "demo");
    }

    #[test]
    fn newer_files_are_refused() {
        let future = json!({"schema_version": 99});
        assert!(matches!(
            migrate(future),
            Err(ProjectError::TooNew {
                found: 99,
                supported: SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn missing_or_zero_version_is_corrupt() {
        assert!(matches!(migrate(json!({})), Err(ProjectError::Corrupt(_))));
        assert!(matches!(
            migrate(json!({"schema_version": 0})),
            Err(ProjectError::Corrupt(_))
        ));
    }

    #[test]
    fn current_version_passes_through() {
        let current = json!({"schema_version": SCHEMA_VERSION, "project": {}});
        assert_eq!(migrate(current.clone()).unwrap(), current);
    }
}
