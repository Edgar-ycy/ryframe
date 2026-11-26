use super::*;

const SID: &str = "s-d85adc8b-4294-4d04-9295-9fd360dd9d90";
const JTI: &str = "058a249946e64354b6f140acf887c15b";
const LOGIN: i64 = 1_790_000_000;
const USER: i64 = 8_042_435_614_521_762_321;

fn subject(hash: &str) -> Subject<'_> {
    Subject {
        scope: "isolated-h",
        tenant: "test-tenant",
        user_id: USER,
        username: "fixture-member",
        client_address: "192.0.2.17",
        login_epoch: LOGIN,
        sid_sha256: hash,
    }
}

fn metadata(login: i64) -> Result<String> {
    let time = OffsetDateTime::from_unix_timestamp(login)?.format(&Rfc3339)?;
    Ok(
        json!({"sid":SID,"tenant_id":"test-tenant","user_id":USER,"username":"fixture-member",
        "dept_name":null,"ipaddr":"192.0.2.17","login_location":null,"browser":null,"os":null,
        "login_time":time,"last_access_time":time,"absolute_exp":LOGIN+90})
        .to_string(),
    )
}

struct Fixture {
    revoked: Option<bool>,
    online: Option<String>,
    indexes: [bool; 4],
    refresh_user_count: Option<&'static str>,
}

impl Fixture {
    fn active() -> Result<Self> {
        Ok(Self {
            revoked: Some(false),
            online: Some(metadata(LOGIN)?),
            indexes: [true; 4],
            refresh_user_count: None,
        })
    }

    fn send(&self, keys: &Keys, parts: &[&str]) -> Result<Value> {
        if parts == ["TIME"] {
            return Ok(json!([(LOGIN + 10).to_string(), "25"]));
        }
        if let Some(index) = keys.indexes.iter().position(|key| key == parts[1]) {
            let member = self.indexes[index];
            return Ok(match parts[0] {
                "TYPE" => json!(if member { "set" } else { "none" }),
                "SCARD" if index == 1 && self.refresh_user_count.is_some() => {
                    json!(self.refresh_user_count)
                }
                "SCARD" | "SISMEMBER" => json!(u8::from(member).to_string()),
                "SMEMBERS" if member => json!([SID]),
                _ => return Err("测试收到未登记的索引命令".into()),
            });
        }
        if parts[1]
            == "ryframe:{isolated-h}:v0.5:refresh-family:s-d85adc8b-4294-4d04-9295-9fd360dd9d90"
        {
            return Ok(match (parts[0], self.revoked) {
                ("TYPE", Some(_)) => json!("hash"),
                ("TYPE", None) => json!("none"),
                ("HMGET", Some(revoked)) => json!([
                    SID,
                    "test-tenant",
                    USER.to_string(),
                    JTI,
                    LOGIN.to_string(),
                    (LOGIN + 90).to_string(),
                    u8::from(revoked).to_string()
                ]),
                ("HMGET", None) => json!([null, null, null, null, null, null, null]),
                ("EXPIRETIME", Some(_)) => json!((LOGIN + 90).to_string()),
                ("EXPIRETIME", None) => json!("-2"),
                _ => return Err("测试收到未登记的会话族命令".into()),
            });
        }
        if parts[1]
            == "ryframe:{isolated-h}:online-user:session:test-tenant:s-d85adc8b-4294-4d04-9295-9fd360dd9d90"
        {
            return Ok(match parts[0] {
                "TYPE" => json!(if self.online.is_some() {
                    "string"
                } else {
                    "none"
                }),
                "GET" => json!(self.online),
                "EXPIRETIME" if self.online.is_some() => json!((LOGIN + 90).to_string()),
                "EXPIRETIME" => json!("-2"),
                _ => return Err("测试收到未登记的在线元数据命令".into()),
            });
        }
        Err("测试收到隔离范围之外的 Redis 命令".into())
    }
}

#[test]
fn active_and_revoked_images_require_four_indexes_and_keep_credentials_private() -> Result<()> {
    let hash = sha256(SID);
    let subject = subject(&hash);
    let keys = Keys::new(&subject)?;
    let mut fixture = Fixture::active()?;
    let readback = read_window(
        &mut |parts| fixture.send(&keys, parts),
        &keys,
        &subject,
        None,
    )?;
    assert!(readback.active && !readback.cleared);
    assert_eq!(readback.sid, SID);
    let proof = readback.proof.to_string();
    for private in [SID, JTI, "fixture-member", "192.0.2.17"] {
        assert!(!proof.contains(private));
    }

    fixture.revoked = Some(true);
    fixture.online = None;
    fixture.indexes = [false; 4];
    let readback = read_window(
        &mut |parts| fixture.send(&keys, parts),
        &keys,
        &subject,
        Some(SID),
    )?;
    assert!(!readback.active && readback.cleared);
    assert_eq!(readback.proof["family"], "revoked");
    fixture.indexes[2] = true;
    let readback = read_window(
        &mut |parts| fixture.send(&keys, parts),
        &keys,
        &subject,
        Some(SID),
    )?;
    assert!(!readback.active && !readback.cleared);
    Ok(())
}

#[test]
fn readback_rejects_multiple_members_identity_drift_and_changed_snapshots() -> Result<()> {
    let hash = sha256(SID);
    let subject = subject(&hash);
    let keys = Keys::new(&subject)?;
    let mut fixture = Fixture::active()?;
    fixture.refresh_user_count = Some("2");
    assert!(
        read_window(
            &mut |parts| fixture.send(&keys, parts),
            &keys,
            &subject,
            None
        )
        .is_err()
    );
    assert!(indexed_sid(Some(SID), Some("other")).is_err());
    fixture.refresh_user_count = None;
    fixture.online = Some(metadata(LOGIN - 1)?);
    assert!(
        read_window(
            &mut |parts| fixture.send(&keys, parts),
            &keys,
            &subject,
            None
        )
        .is_err()
    );
    fixture.online = Some(metadata(LOGIN)?);
    let mut reads = 0;
    let mut read = |parts: &[&str]| {
        if parts[0] == "GET" {
            reads += 1;
            if reads == 2 {
                let mut value: Value = serde_json::from_str(fixture.online.as_deref().unwrap())?;
                value["browser"] = json!("other");
                return Ok(json!(value.to_string()));
            }
        }
        fixture.send(&keys, parts)
    };
    assert!(read_window(&mut read, &keys, &subject, None).is_err());
    assert!(require_sid(&subject, "s-a4ed1d64-1ecd-4c9e-ada2-c323bb5b7a28").is_err());
    Ok(())
}
