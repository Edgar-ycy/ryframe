pub(super) fn source(name: &str, field: &str, permission: &str, relations: &[&str]) -> String {
    let relations = relations
        .iter()
        .map(|target| {
            format!(
                "[[relations]]\nname = \"{target}\"\nkind = \"belongs_to\"\nlocal_field = \"{field}\"\ntarget_resource = \"{target}\"\n"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "schema_version = 1\n\
         [[fields]]\nname = \"{field}\"\nvalue_type = \"string\"\norder = 1\n\
         {relations}\n\
         [resource]\nname = \"{name}\"\nmodule = \"{name}\"\nprofile = \"system_control\"\n\
         [database]\ntable = \"{name}\"\nprimary_key = [\"{field}\"]\n\
         [access]\ncapability = \"{permission}\"\n"
    )
}
