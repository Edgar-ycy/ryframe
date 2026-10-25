// 运行校验与离线导出断言共同使用的实际结构查询。
pub(super) const TABLE: &str = "SELECT t.table_name AS `table_name`, t.engine AS `engine`, \
                c.character_set_name AS `character_set_name`, \
                t.table_collation AS `table_collation` \
         FROM information_schema.tables t \
         INNER JOIN information_schema.collation_character_set_applicability c \
           ON c.collation_name = t.table_collation \
         WHERE t.table_schema = DATABASE() AND t.table_type = 'BASE TABLE' \
           AND t.table_name = ? LIMIT 1";

pub(super) const COLUMNS: &str = "SELECT ordinal_position AS `schema_order`, column_name AS `column_name`, column_type AS `column_type`, \
                is_nullable AS `is_nullable`, character_set_name AS `character_set_name`, \
                collation_name AS `collation_name`, column_key AS `column_key`, \
                column_default AS `column_default`, extra AS `extra`, \
                generation_expression AS `generation_expression` \
         FROM information_schema.columns WHERE table_schema = DATABASE() \
           AND table_name = ? ORDER BY ordinal_position";

pub(super) const INDEXES: &str = "SELECT ROW_NUMBER() OVER (ORDER BY index_name, seq_in_index) AS `schema_order`, index_name AS `index_name`, column_name AS `column_name`, \
         CAST(seq_in_index AS SIGNED) AS `seq_in_index`, \
         CAST(non_unique AS SIGNED) AS `non_unique`, index_type AS `index_type`, \
         CAST(sub_part AS SIGNED) AS `sub_part`, is_visible AS `is_visible` \
         FROM information_schema.statistics WHERE table_schema = DATABASE() \
           AND table_name = ? ORDER BY index_name, seq_in_index";

pub(super) const CONSTRAINTS: &str = "SELECT ROW_NUMBER() OVER (ORDER BY constraint_name) AS `schema_order`, constraint_name AS `constraint_name`, constraint_type AS `constraint_type`, \
                enforced AS `enforced` \
         FROM information_schema.table_constraints WHERE table_schema = DATABASE() \
           AND table_name = ? ORDER BY constraint_name";

pub(super) const CHECKS: &str = "SELECT ROW_NUMBER() OVER (ORDER BY tc.constraint_name) AS `schema_order`, tc.constraint_name AS `constraint_name`, cc.check_clause AS `check_clause` \
         FROM information_schema.table_constraints tc \
         INNER JOIN information_schema.check_constraints cc \
           ON cc.constraint_schema = tc.constraint_schema \
          AND cc.constraint_name = tc.constraint_name \
         WHERE tc.table_schema = DATABASE() AND tc.table_name = ? \
           AND tc.constraint_type = 'CHECK' ORDER BY tc.constraint_name";

pub(super) const FOREIGN_KEYS: &str = "SELECT ROW_NUMBER() OVER (ORDER BY k.constraint_name, k.ordinal_position) AS `schema_order`, k.constraint_name AS `constraint_name`, k.column_name AS `column_name`, \
                CAST(k.ordinal_position AS SIGNED) AS `ordinal_position`, \
                k.referenced_table_schema AS `referenced_table_schema`, \
                DATABASE() AS `current_schema`, \
                k.referenced_table_name AS `referenced_table_name`, \
                k.referenced_column_name AS `referenced_column_name`, \
                r.update_rule AS `update_rule`, r.delete_rule AS `delete_rule` \
         FROM information_schema.key_column_usage k \
         INNER JOIN information_schema.referential_constraints r \
           ON r.constraint_schema = k.constraint_schema \
          AND r.table_name = k.table_name \
          AND r.constraint_name = k.constraint_name \
         WHERE k.table_schema = DATABASE() AND k.table_name = ? \
           AND k.referenced_table_name IS NOT NULL \
         ORDER BY k.constraint_name, k.ordinal_position";
