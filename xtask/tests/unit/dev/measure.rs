use super::rustc_compiles;

#[test]
fn cancellation_requires_compile_arguments_and_rejects_rustc_probes() {
    for (arguments, expected) in [
        (
            &["--crate-name", "ryframe", "--emit=dep-info,link"][..],
            true,
        ),
        (&["--crate-name=ryframe", "--emit", "metadata"][..], true),
        (&["-vV"][..], false),
        (&["--version"][..], false),
        (&["--crate-name", "___", "--print=file-names"][..], false),
        (
            &["--crate-name", "___", "--emit=metadata", "--print", "cfg"][..],
            false,
        ),
        (&["--crate-name", "ryframe"][..], false),
        (&["--emit=metadata"][..], false),
        (&["--crate-name", "ryframe", "--emit="][..], false),
        (&["--crate-name", "--emit=metadata"][..], false),
    ] {
        let argv = ["rustc.exe"]
            .into_iter()
            .chain(arguments.iter().copied())
            .map(str::to_owned)
            .collect::<Vec<_>>();

        assert_eq!(rustc_compiles(&argv), expected, "{arguments:?}");
    }
}
