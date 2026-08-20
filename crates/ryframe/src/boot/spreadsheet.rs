use std::sync::Arc;

use ryframe_adapters::excel::{ExcelArtifact, IncrementalExcelWriter};
use ryframe_application::{
    SpreadsheetArtifact, SpreadsheetBatchProgress, SpreadsheetRow, SpreadsheetWriter,
    SpreadsheetWriterFactory,
};
use ryframe_kernel::AppResult;

struct SpreadsheetFactoryBridge;

struct SpreadsheetWriterBridge {
    writer: IncrementalExcelWriter<'static>,
}

struct SpreadsheetArtifactBridge {
    artifact: ExcelArtifact,
}

impl SpreadsheetWriterFactory for SpreadsheetFactoryBridge {
    fn create(
        &self,
        sheet_name: &'static str,
        headers: &'static [(&'static str, &'static str)],
    ) -> AppResult<Box<dyn SpreadsheetWriter>> {
        IncrementalExcelWriter::new(sheet_name, headers).map(|writer| {
            Box::new(SpreadsheetWriterBridge { writer }) as Box<dyn SpreadsheetWriter>
        })
    }
}

impl SpreadsheetWriter for SpreadsheetWriterBridge {
    fn data_rows(&self) -> u64 {
        self.writer.data_rows()
    }

    fn input_bytes(&self) -> u64 {
        self.writer.input_bytes()
    }

    fn append_rows(
        &mut self,
        rows: &mut dyn Iterator<Item = SpreadsheetRow>,
    ) -> AppResult<SpreadsheetBatchProgress> {
        self.writer
            .append_rows(rows)
            .map(|progress| SpreadsheetBatchProgress {
                batch_rows: progress.batch_rows,
                total_rows: progress.total_rows,
                total_input_bytes: progress.total_input_bytes,
            })
    }

    fn finish(self: Box<Self>) -> AppResult<Box<dyn SpreadsheetArtifact>> {
        self.writer.finish().map(|artifact| {
            Box::new(SpreadsheetArtifactBridge { artifact }) as Box<dyn SpreadsheetArtifact>
        })
    }
}

impl SpreadsheetArtifact for SpreadsheetArtifactBridge {
    fn path(&self) -> &std::path::Path {
        self.artifact.path()
    }

    fn size(&self) -> u64 {
        self.artifact.size()
    }

    fn data_rows(&self) -> u64 {
        self.artifact.data_rows()
    }

    fn input_bytes(&self) -> u64 {
        self.artifact.input_bytes()
    }
}

pub fn writer_factory() -> Arc<dyn SpreadsheetWriterFactory> {
    Arc::new(SpreadsheetFactoryBridge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_streaming_artifact_without_buffering_rows() {
        let mut writer = writer_factory()
            .create("测试", &[("id", "编号")])
            .expect("应创建表格写入器");
        let mut rows = std::iter::empty::<SpreadsheetRow>();
        let progress = writer.append_rows(&mut rows).expect("空批次应可写入");
        assert_eq!(progress.total_rows, 0);

        let artifact = writer.finish().expect("应生成表格制品");
        assert!(artifact.path().is_file());
        assert!(artifact.size() > 0);
        assert_eq!(artifact.data_rows(), 0);
    }
}
