use super::*;

const ACTOR: &str = "test-operator";
const SID: &str = "s-d85adc8b-4294-4d04-9295-9fd360dd9d90";
const REQUEST: &str = "01923843-5ca0-7000-8000-000000000001";
const EVENT: &str = "01923843-5ca1-7000-8000-000000000002";
const DIFFERENT_EVENT: &str = "01923843-5ca2-7000-8000-000000000003";

fn at(offset: i64) -> Result<String> {
    let seconds = uuid7_millis(REQUEST)?.div_euclid(1000) + offset;
    Ok(OffsetDateTime::from_unix_timestamp(seconds)?.format(&Rfc3339)?)
}

fn record(audit: &Audit<'_>, source: &str, event: &str, path: &str, offset: i64) -> Result<String> {
    Ok(format!(
        "{source}\t{event}\t{REQUEST}\t{}\t{path}\t{}\t1",
        at(offset)?,
        audit.actor_sha256,
    ))
}

#[test]
fn confirmed_request_matches_template_or_exact_path_and_deduplicates_its_event() -> Result<()> {
    let audit = Audit::new(ACTOR, SID, Some(REQUEST), &at(-1)?)?;
    let outbox = record(&audit, "outbox", EVENT, &audit.template_sha256, 1)?;
    let projection = record(&audit, "projection", EVENT, &audit.template_sha256, 2)?;
    let proof = audit.decode(&outbox)?;
    assert_eq!(proof["path_kind"], "route-template");
    assert!(proof["projection_observed_at"].is_null());
    let proof = audit.decode(&format!("{outbox}\n{projection}"))?;
    assert_eq!(proof["event_id"], EVENT);
    assert_eq!(proof["projection_observed_at"], at(2)?);
    assert!(!proof.to_string().contains(SID));
    assert!(!proof.to_string().contains(ACTOR));

    let matching_path = record(&audit, "outbox", EVENT, &audit.concrete_sha256, 1)?;
    assert_eq!(audit.decode(&matching_path)?["path_kind"], "concrete");
    let sql = audit_query(audit.request_id);
    for private in [SID, ACTOR, "oper_param", "json_result"] {
        assert!(!sql.contains(private));
    }
    Ok(())
}

#[test]
fn ambiguous_failed_or_unattributed_audits_are_rejected() -> Result<()> {
    let audit = Audit::new(ACTOR, SID, Some(REQUEST), &at(-1)?)?;
    let outbox = record(&audit, "outbox", EVENT, &audit.template_sha256, 1)?;
    let mismatch = record(
        &audit,
        "projection",
        DIFFERENT_EVENT,
        &audit.template_sha256,
        2,
    )?;
    assert!(audit.decode(&format!("{outbox}\n{mismatch}")).is_err());
    assert!(audit.decode(&format!("{outbox}\n{outbox}")).is_err());
    let failed = outbox.strip_suffix('1').unwrap().to_string() + "0";
    assert!(audit.decode(&failed).is_err());
    let wrong_path = record(
        &audit,
        "outbox",
        EVENT,
        &sha256("/api/v1/system/online/other"),
        1,
    )?;
    assert!(audit.decode(&wrong_path).is_err());
    let before = record(&audit, "outbox", EVENT, &audit.template_sha256, -2)?;
    assert!(audit.decode(&before).is_err());
    assert!(Audit::new(ACTOR, SID, None, &at(-1)?).is_err());
    assert!(
        Audit::new(
            ACTOR,
            SID,
            Some("01923843-5ca0-4000-8000-000000000001"),
            &at(-1)?
        )
        .is_err()
    );
    assert!(Audit::new(ACTOR, SID, Some(REQUEST), &at(1)?).is_err());
    Ok(())
}
