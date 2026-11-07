//! 迁自目标续作 Python 测试的只读观察场景，不连接数据库或 Redis。
use super::*;
use std::{fs, path::PathBuf};

struct Fixture {
    backend: PathBuf,
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let backend = crate::workspace::root_dir();
        let parent = backend.join(".local-tests/target-resume-observations");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("中文 空格-")
            .tempdir_in(parent)
            .unwrap();
        Self { backend, directory }
    }

    fn receipt(&self, stage: &str, index: usize, command: Value, stdout: &str) -> String {
        let name = format!("{stage}-{index:032x}.command.json");
        self.put(
            &name,
            &json!({"command":command,"returncode":0,"error_type":null,
            "stdout":stdout,"stderr":""}),
        );
        name
    }

    fn put(&self, name: &str, value: &Value) {
        fs::write(
            self.directory.path().join(name),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }

    fn change_stdout(&self, name: &str, stdout: &str) {
        let mut value = json_io::read(&self.directory.path().join(name)).unwrap();
        value["stdout"] = json!(stdout);
        self.put(name, &value);
    }
}

#[test]
fn windows_redis_resume_consumes_exact_projection_rounds_and_rejects_mixed_evidence() {
    let fixture = Fixture::new();
    let tool = json!({"platform":"windows","path":"D:/tools/redis-server.exe","sha256":"a".repeat(64),
        "version":"Redis server v=7.4.11 bits=64","dlls":{"cygwin1.dll":"b".repeat(64)}});
    let original = json!({"platform":"windows","port":29770,"pid":36784,"started":"123",
        "executable":tool["path"],"sha256":tool["sha256"],"run_id":"a".repeat(40),"tool":tool,
        "configuration":{"path":"D:/缓存 数据/redis.conf","bytes":5,"sha256":"c".repeat(64)}});
    let mut current = original.clone();
    current["pid"] = json!(36786);
    current["started"] = json!("124");
    current["run_id"] = json!("b".repeat(40));
    let name = |index| format!("redis-windows-{index:032x}.json");
    for index in 0..5 {
        fixture.put(&name(index), &current);
    }
    let baseline = BTreeSet::from([name(0), name(1)]);
    let mut rounds = BTreeSet::from([name(2), name(3), name(4)]);
    let run = |names: &mut BTreeSet<String>, count| {
        redis_delta(
            &fixture.backend,
            fixture.directory.path(),
            &baseline,
            names,
            &original,
            count,
        )
    };
    assert!(run(&mut rounds.clone(), 2).is_err());
    run(&mut rounds.clone(), 3).unwrap();
    rounds.insert(fixture.receipt("redis-kernel", 9, json!(["wsl.exe"]), "linux"));
    assert!(run(&mut rounds, 3).is_err());
    let mut drift = current.clone();
    drift["run_id"] = original["run_id"].clone();
    fixture.put(&name(3), &drift);
    let mut rounds = BTreeSet::from([name(2), name(3), name(4)]);
    assert!(run(&mut rounds, 3).is_err());
    fixture.put(&name(3), &current);
    run(&mut rounds, 3).unwrap();
    assert!(rounds.is_empty());

    let mut swapped = current.clone();
    swapped["tool"]["dlls"]["cygwin1.dll"] = json!("d".repeat(64));
    assert!(prepare_observations::windows_redis_transition(&original, &swapped).is_err());
    let command = format!("redis-windows-{:032x}.command.json", 0);
    assert!(super::super::evidence_prepare::artifact(&name(0)).unwrap());
    assert!(!super::super::evidence_prepare::artifact(&command).unwrap());
}

#[test]
fn mysql_resume_observations_bind_exact_before_after_multiset_without_writes() {
    let fixture = Fixture::new();
    let request = json!({"target":{"databases":[{"key":"shared","server_uuid":"uuid","database":"business"}]}});
    let command = json!([
        "mysql",
        "--defaults-file=中文 空格.cnf",
        "--execute",
        "show"
    ]);
    let name = fixture.receipt("mysql-shared", 0, command.clone(), "uuid\r\n");
    let baseline = BTreeSet::from([name]);
    let mut current = BTreeSet::new();
    for (index, stdout) in [
        "uuid\r\n",
        "uuid\n",
        "",
        "",
        "",
        "",
        "uuid\nbusiness\n",
        "uuid\r\nbusiness\r\n",
        "uuid\nbusiness\n",
    ]
    .iter()
    .enumerate()
    {
        current.insert(fixture.receipt("mysql-shared", index + 1, command.clone(), stdout));
    }
    let names = current.clone();
    let before = fs::read(fixture.directory.path().join(current.last().unwrap())).unwrap();
    mysql(
        &fixture.backend,
        fixture.directory.path(),
        &baseline,
        &mut current,
        &request,
    )
    .unwrap();
    assert!(current.is_empty());
    assert_eq!(
        fs::read(fixture.directory.path().join(names.last().unwrap())).unwrap(),
        before
    );
    for stdout in [
        "uuid\nbusiness\nunexpected",
        "uuid\n",
        "uuid\nbusiness\n\n",
        " uuid\nbusiness\n",
    ] {
        fixture.change_stdout(names.last().unwrap(), stdout);
        let error = mysql(
            &fixture.backend,
            fixture.directory.path(),
            &baseline,
            &mut names.clone(),
            &request,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "fresh 初始化 MySQL 前后像输出多重集无效");
    }
}

fn proc_stat(pid: u32, name: &str, start: &str, volatile: bool) -> String {
    let mut fields = vec!["0"; 20];
    fields[0] = if volatile { "R" } else { "S" };
    fields[11] = if volatile { "41" } else { "0" };
    fields[12] = if volatile { "73" } else { "0" };
    fields[19] = start;
    format!("{pid} ({name}) {}\n", fields.join(" "))
}

#[test]
fn redis_resume_allows_only_volatile_proc_stat_changes_across_fifteen_rounds() {
    let fixture = Fixture::new();
    let mut baseline = BTreeSet::new();
    let mut current = BTreeSet::new();
    let mut stat = String::new();
    let command = |kind| {
        if kind == 0 {
            json!(["/usr/bin/cat", "/proc/200/stat"])
        } else {
            json!(["/usr/bin/cat", format!("/fixture/{kind}")])
        }
    };
    for kind in 0..5 {
        for round in 0..17 {
            let stdout = if kind == 0 {
                proc_stat(200, "redis-server", "67890", round == 16)
            } else {
                format!("stable-{kind}\n")
            };
            let name = fixture.receipt("redis-kernel", kind * 17 + round, command(kind), &stdout);
            if round < 2 {
                baseline.insert(name);
            } else {
                if kind == 0 && round == 16 {
                    stat = name.clone();
                }
                current.insert(name);
            }
        }
    }
    redis(
        &fixture.backend,
        fixture.directory.path(),
        &baseline,
        &current,
        15,
    )
    .unwrap();
    for stdout in [
        proc_stat(201, "redis-server", "67890", true),
        proc_stat(200, "other", "67890", true),
        proc_stat(200, "redis-server", "67891", true),
    ] {
        fixture.change_stdout(&stat, &stdout);
        assert_eq!(
            redis(
                &fixture.backend,
                fixture.directory.path(),
                &baseline,
                &current,
                15
            )
            .unwrap_err()
            .to_string(),
            "fresh 初始化 Redis 工具、配置或进程身份发生变化"
        );
    }
    fixture.change_stdout(&stat, &proc_stat(200, "redis-server", "67890", true));
    assert!(
        redis(
            &fixture.backend,
            fixture.directory.path(),
            &baseline,
            &current,
            14
        )
        .is_err()
    );
    current.remove(&stat);
    assert!(
        redis(
            &fixture.backend,
            fixture.directory.path(),
            &baseline,
            &current,
            15
        )
        .is_err()
    );
}
