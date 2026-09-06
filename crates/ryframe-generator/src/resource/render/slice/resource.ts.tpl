function {name}Resource(translate: Translate) {{
  return defineFlatCrudResource<
    {pascal}Record,
    {pascal}Query,
    {pascal}Form,
    {pascal}CreateInput,
    {pascal}UpdateInput
  >({{
    key: {name:?},
    initialQuery: (): {pascal}Query => ({{
      page: 1,
      page_size: 10,
{query_defaults}    }}),
    emptyForm: (): {pascal}Form => ({{
{empty_form}    }}),
    editForm: record => ({{
{edit_form}    }}),
    createInput: form => ({{
{create_input}    }}),
    updateInput: form => ({{
{update_input}    }}),
    recordId: record => String(record.{record_id}),
    messages: {{
      addSuccess: translate("新增成功", "Created"),
      addTitle: translate("新增{zh}", "Add {en}"),
      deleteConfirm: record => translate(
        `确定删除 ${{String(record.{record_label})}} 吗？`,
        `Delete ${{String(record.{record_label})}}?`,
      ),
      deleteSuccess: translate("删除成功", "Deleted"),
      detailMissing: translate("资源不存在", "Resource not found"),
      editTitle: translate("编辑{zh}", "Edit {en}"),
      updateSuccess: translate("更新成功", "Updated"),
      warningTitle: translate("提示", "Warning"),
    }},
    adapter: {{
      async list(query, signal) {{
        const response = await list{pascal}({{ ...query }}, signal)
        return response.data ?? emptyPageResponse<{pascal}Record>(query)
      }},
      async detail(id, signal) {{
        const response = await get{pascal}(id, signal)
        if (!response.data) throw new Error(translate("资源不存在", "Resource not found"))
        return response.data
      }},
      async create(input) {{ await create{pascal}(input) }},
      async update(id, input) {{ await update{pascal}(id, input) }},
      async remove(id) {{ await delete{pascal}(id) }},
    }},
  }})
}}
