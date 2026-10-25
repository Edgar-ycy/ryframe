use super::{choose, get, json, number, set};

// 与 strip_redundant_outer_parentheses、redundant_atomic_group 和 atomic_group 同序。
// 逐次移除第一个可消除组，不能同时删除所有组，例如 (a)(b) 第二组会受首组移除影响。
pub(super) fn cte() -> String {
    let mut branches = outer();
    branches.extend(seek());
    branches.extend(analyze());
    let step = choose(&branches, &set(&[("m", "'d'".into())]));
    format!(
        "paren_state(state) AS (SELECT JSON_OBJECT('t', x, 'm', 'o', 'p', 0, 'd', 0, 'b', 0, 'o', 0, 'c', 0, 'stack', JSON_ARRAY()) FROM lexed UNION ALL SELECT {step} FROM paren_state WHERE {} <> 'd'), normalized AS (SELECT {} AS x FROM paren_state WHERE {}='d')",
        get("m"),
        json("t"),
        get("m")
    )
}

fn count() -> String {
    format!("JSON_LENGTH({})", json("t"))
}
fn token(index: &str, field: usize) -> String {
    format!(
        "IF(({index})>=0 AND ({index})<{}, JSON_UNQUOTE(JSON_EXTRACT({}, CONCAT('$[', ({index}), '][{field}]'))), '')",
        count(),
        json("t")
    )
}
fn is(index: &str, kind: &str, hex: &str) -> String {
    format!(
        "({}='{kind}' AND {}='{hex}')",
        token(index, 0),
        token(index, 1)
    )
}
fn advance() -> String {
    set(&[("p", format!("{}+1", number("p")))])
}
fn seeking() -> String {
    set(&[
        ("m", "'s'".into()),
        ("p", "0".into()),
        ("d", "0".into()),
        ("stack", "JSON_ARRAY()".into()),
    ])
}

fn outer() -> Vec<(String, String)> {
    let mode = get("m");
    let p = number("p");
    let n = count();
    let depth = format!(
        "{} + IF({},1,IF({},-1,0))",
        number("d"),
        is(&p, "p", "28"),
        is(&p, "p", "29")
    );
    vec![
        (
            format!(
                "{mode}='o' AND NOT ({} AND {})",
                is("0", "p", "28"),
                is(&format!("{n}-1"), "p", "29")
            ),
            seeking(),
        ),
        (
            format!("{mode}='o' AND (({depth})<0 OR (({depth})=0 AND {p}<{n}-1))"),
            seeking(),
        ),
        (
            format!("{mode}='o' AND {p}={n}-1 AND ({depth})=0"),
            set(&[
                (
                    "t",
                    format!(
                        "JSON_REMOVE({}, CONCAT('$[', {n}-1, ']'), '$[0]')",
                        json("t")
                    ),
                ),
                ("p", "0".into()),
                ("d", "0".into()),
            ]),
        ),
        (format!("{mode}='o' AND {p}>={n}-1"), seeking()),
        (
            format!("{mode}='o'"),
            set(&[("p", format!("{p}+1")), ("d", depth)]),
        ),
    ]
}

fn seek() -> Vec<(String, String)> {
    let mode = get("m");
    let p = number("p");
    let stack = json("stack");
    let size = format!("JSON_LENGTH({stack})");
    let open =
        format!("CAST(JSON_UNQUOTE(JSON_EXTRACT({stack}, CONCAT('$[', {size}-1, ']'))) AS SIGNED)");
    let popped = format!("JSON_REMOVE({stack}, CONCAT('$[', {size}-1, ']'))");
    let preceding = format!(
        "{}='i' OR {}",
        token(&format!("{open}-1"), 0),
        is(&format!("{open}-1"), "k", "696E")
    );
    vec![
        (
            format!("{mode}='s' AND {p}>={}", count()),
            set(&[("m", "'d'".into())]),
        ),
        (
            format!("{mode}='s' AND {}", is(&p, "p", "28")),
            set(&[
                ("p", format!("{p}+1")),
                ("stack", format!("JSON_ARRAY_APPEND({stack}, '$', {p})")),
            ]),
        ),
        (
            format!("{mode}='s' AND {} AND {size}=0", is(&p, "p", "29")),
            set(&[("m", "'d'".into())]),
        ),
        (
            format!("{mode}='s' AND {} AND ({preceding})", is(&p, "p", "29")),
            set(&[("p", format!("{p}+1")), ("stack", popped.clone())]),
        ),
        (
            format!("{mode}='s' AND {}", is(&p, "p", "29")),
            set(&[
                ("m", "'a'".into()),
                ("o", open.clone()),
                ("c", p.clone()),
                ("p", format!("{open}+1")),
                ("d", "0".into()),
                ("b", "0".into()),
                ("stack", popped),
            ]),
        ),
        (format!("{mode}='s'"), advance()),
    ]
}

fn analyze() -> Vec<(String, String)> {
    let mode = get("m");
    let p = number("p");
    let d = number("d");
    let and = is(&p, "k", "616E64");
    let rejected = format!("{and} OR {} OR {}", is(&p, "k", "6F72"), is(&p, "s", "2C"));
    vec![
        (
            format!("{mode}='a' AND {p}>={}", number("c")),
            set(&[
                (
                    "t",
                    format!(
                        "JSON_REMOVE({}, CONCAT('$[', {}, ']'), CONCAT('$[', {}, ']'))",
                        json("t"),
                        number("c"),
                        number("o")
                    ),
                ),
                ("m", "'s'".into()),
                ("p", "0".into()),
                ("stack", "JSON_ARRAY()".into()),
            ]),
        ),
        (
            format!("{mode}='a' AND {}", is(&p, "p", "28")),
            set(&[("p", format!("{p}+1")), ("d", format!("{d}+1"))]),
        ),
        (
            format!("{mode}='a' AND {}", is(&p, "p", "29")),
            set(&[("p", format!("{p}+1")), ("d", format!("GREATEST({d}-1,0)"))]),
        ),
        (
            format!("{mode}='a' AND {d}=0 AND {}", is(&p, "k", "6265747765656E")),
            set(&[("p", format!("{p}+1")), ("b", "1".into())]),
        ),
        (
            format!("{mode}='a' AND {d}=0 AND {and} AND {}=1", number("b")),
            set(&[("p", format!("{p}+1")), ("b", "0".into())]),
        ),
        (
            format!("{mode}='a' AND {d}=0 AND ({rejected})"),
            set(&[("m", "'s'".into()), ("p", format!("{}+1", number("c")))]),
        ),
        (format!("{mode}='a'"), advance()),
    ]
}
