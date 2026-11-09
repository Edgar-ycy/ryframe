use super::super::rules;
use super::{choose, get, json, literal, lower, number, set};

pub(super) fn cte() -> String {
    let mut branches = quoted();
    branches.extend(top());
    let step = choose(
        &branches,
        &token(&literal("s"), &format!("HEX({})", byte()), "1"),
    );
    format!(
        "lex(state) AS (SELECT JSON_OBJECT('p', 1, 'm', 't', 'q', 0, 'e', 0, 'b', '', 't', JSON_ARRAY()) UNION ALL SELECT {step} FROM lex CROSS JOIN src WHERE {} <= OCTET_LENGTH(raw) OR {} <> 't'), lexed AS (SELECT {} AS x FROM lex CROSS JOIN src WHERE {} > OCTET_LENGTH(raw) AND {} = 't')",
        number("p"),
        get("m"),
        json("t"),
        number("p"),
        get("m")
    )
}

fn byte() -> String {
    format!("SUBSTRING(raw, {}, 1)", number("p"))
}
fn next() -> String {
    format!("SUBSTRING(raw, {}+1, 1)", number("p"))
}
fn tail() -> String {
    format!("SUBSTRING(raw, {})", number("p"))
}
fn append(hex: &str, consumed: &str) -> String {
    set(&[
        ("p", format!("{}+({consumed})", number("p"))),
        ("b", format!("CONCAT({}, {hex})", get("b"))),
    ])
}
fn token(kind: &str, hex: &str, consumed: &str) -> String {
    set(&[
        ("p", format!("{}+({consumed})", number("p"))),
        ("m", "'t'".into()),
        ("b", "''".into()),
        (
            "t",
            format!(
                "JSON_ARRAY_APPEND({}, '$', JSON_ARRAY({kind}, {hex}))",
                json("t")
            ),
        ),
    ])
}

fn quoted() -> Vec<(String, String)> {
    let b = byte();
    let next = next();
    let p = number("p");
    let q = number("q");
    let buf = get("b");
    let quote = format!("HEX(CHAR({q} USING ascii))");
    let run = format!(
        "CHAR_LENGTH(REGEXP_SUBSTR(CONVERT({} USING latin1), {}))",
        tail(),
        literal(r"^\\+")
    );
    let after_run = format!("ORD(SUBSTRING(raw, {p}+{run}, 1)) = {q}");
    vec![
        (
            format!("{p} > OCTET_LENGTH(raw) AND {} = 'i'", get("m")),
            token(
                "'i'",
                &format!("HEX({})", lower(&format!("UNHEX({buf})"))),
                "0",
            ),
        ),
        (
            format!("{p} > OCTET_LENGTH(raw) AND {} = 'l'", get("m")),
            token("'l'", &buf, "0"),
        ),
        (
            format!("{} = 'i' AND ORD({b}) = 96 AND ORD({next}) = 96", get("m")),
            append("'60'", "2"),
        ),
        (
            format!("{} = 'i' AND ORD({b}) = 96", get("m")),
            token(
                "'i'",
                &format!("HEX({})", lower(&format!("UNHEX({buf})"))),
                "1",
            ),
        ),
        (
            format!("{} = 'i'", get("m")),
            append(&format!("HEX({b})"), "1"),
        ),
        (
            format!(
                "{}='l' AND {}=1 AND ORD({b})=92 AND {after_run} AND {run}=1",
                get("m"),
                number("e")
            ),
            token(
                "'l'",
                &format!("CONCAT({buf}, {quote})"),
                &format!("{run}+1"),
            ),
        ),
        (
            format!(
                "{}='l' AND {}=1 AND ORD({b})=92 AND {after_run} AND {run}>=3 AND MOD({run},2)=1",
                get("m"),
                number("e")
            ),
            append(
                &format!("CONCAT(REPEAT('5C', ({run}-3)/2), {quote}, {quote})"),
                &format!("{run}+1"),
            ),
        ),
        (
            format!("{}='l' AND {}=1 AND ORD({b})=92", get("m"), number("e")),
            append(&format!("REPEAT('5C', {run})"), &run),
        ),
        (
            format!(
                "{}='l' AND {}=0 AND ((ORD({b})=92 AND ORD({next})={q}) OR (ORD({b})={q} AND ORD({next})={q}))",
                get("m"),
                number("e")
            ),
            append(&format!("CONCAT({quote}, {quote})"), "2"),
        ),
        (
            format!("{}='l' AND ORD({b})={q}", get("m")),
            token("'l'", &format!("CONCAT({buf}, {quote})"), "1"),
        ),
        (
            format!("{}='l'", get("m")),
            append(&format!("HEX({b})"), "1"),
        ),
    ]
}

fn top() -> Vec<(String, String)> {
    let p = number("p");
    let b = byte();
    let next = next();
    let mut branches = vec![(
        format!("ORD({b}) IN (9,10,12,13,32)"),
        set(&[("p", format!("{p}+1"))]),
    )];
    for introducer in rules::INTRODUCERS {
        let end = format!("{p}+{}", introducer.len());
        let follows = format!("SUBSTRING(raw, {end}, 1)");
        branches.push((format!("CAST({} AS BINARY)=CAST({} AS BINARY) AND (ORD({follows}) IN (34,39) OR (ORD({follows})=92 AND ORD(SUBSTRING(raw, {end}+1, 1)) IN (34,39)))", lower(&format!("SUBSTRING(raw, {p}, {})", introducer.len())), literal(introducer)), set(&[("p", end)])));
    }
    let escaped = format!("ORD({b})=92");
    branches.push((
        format!("ORD({b}) IN (34,39) OR ({escaped} AND ORD({next}) IN (34,39))"),
        set(&[
            ("p", format!("{p}+IF({escaped},2,1)")),
            ("m", "'l'".into()),
            ("q", format!("IF({escaped},ORD({next}),ORD({b}))")),
            ("e", format!("IF({escaped},1,0)")),
            ("b", format!("IF({escaped},HEX({next}),HEX({b}))")),
        ]),
    ));
    branches.push((
        format!("ORD({b})=96"),
        set(&[
            ("p", format!("{p}+1")),
            ("m", "'i'".into()),
            ("b", "''".into()),
        ]),
    ));
    branches.extend(numbers_and_words());
    for symbol in rules::SYMBOLS {
        branches.push((
            format!(
                "SUBSTRING(raw, {p}, {})=CAST({} AS BINARY)",
                symbol.len(),
                literal(symbol)
            ),
            token(
                "'s'",
                &format!("HEX({})", literal(symbol)),
                &symbol.len().to_string(),
            ),
        ));
    }
    branches.push((
        format!("ORD({b}) IN (40,41)"),
        token("'p'", &format!("HEX({b})"), "1"),
    ));
    branches
}

fn numbers_and_words() -> Vec<(String, String)> {
    let b = byte();
    let next = next();
    let numeric = format!(
        "CHAR_LENGTH(REGEXP_SUBSTR(CONVERT({} USING latin1), {}))",
        tail(),
        literal(r"^(0[xXbB][0-9a-fA-F]*|[0-9]*([.][0-9]*)?([eE][+-]?[0-9]+)?)")
    );
    let word = format!(
        "CHAR_LENGTH(REGEXP_SUBSTR(CONVERT({} USING latin1), {}))",
        tail(),
        literal("^([A-Za-z0-9_$]|[^\0-\u{7f}])+")
    );
    let value = |size: &str| lower(&format!("SUBSTRING(raw, {}, {size})", number("p")));
    let lowered = value(&word);
    let keywords = rules::KEYWORDS
        .iter()
        .map(|s| format!("CAST({} AS BINARY)", literal(s)))
        .collect::<Vec<_>>()
        .join(",");
    vec![
        (
            format!(
                "ORD({b}) BETWEEN 48 AND 57 OR (ORD({b})=46 AND ORD({next}) BETWEEN 48 AND 57)"
            ),
            token("'n'", &format!("HEX({})", value(&numeric)), &numeric),
        ),
        (
            format!(
                "ORD({b}) BETWEEN 65 AND 90 OR ORD({b}) BETWEEN 97 AND 122 OR ORD({b}) BETWEEN 48 AND 57 OR ORD({b}) IN (95,36) OR ORD({b})>=128"
            ),
            token(
                &format!("IF(CAST({lowered} AS BINARY) IN ({keywords}), 'k', 'i')"),
                &format!("HEX({lowered})"),
                &word,
            ),
        ),
    ]
}
