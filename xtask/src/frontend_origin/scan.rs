//! 完整来源逐项读取：最多四个短生命周期线程，按输入顺序合并。
use crate::{Result, recovery::runtime::dataset_authority::reads};

pub(crate) fn ordered<T: Sync, R: Send>(
    items: &[T],
    read: impl Fn(&T) -> Result<R> + Sync,
) -> Result<Vec<R>> {
    if reads::is_active() || items.len() < 2 {
        return items.iter().map(read).collect();
    }
    std::thread::scope(|scope| {
        let read = &read;
        let handles = items
            .chunks(items.len().div_ceil(4))
            .map(|chunk| {
                std::thread::Builder::new().spawn_scoped(scope, move || {
                    // 非 Send 错误只在所属线程消费；不传递文件内容或 panic 载荷。
                    chunk
                        .iter()
                        .map(|item| read(item).map_err(|error| error.to_string()))
                        .collect::<std::result::Result<Vec<_>, String>>()
                })
            })
            .collect::<Vec<_>>();
        // 必须逐一 join 后再返回错误，避免 scope 的自动 join 将 panic 再次抛出。
        let batches = handles
            .into_iter()
            .map(|handle| match handle {
                Ok(handle) => handle
                    .join()
                    .unwrap_or_else(|_| Err("来源读取线程异常终止".into())),
                Err(_) => Err("无法创建来源读取线程".into()),
            })
            .collect::<Vec<_>>();
        let mut output = Vec::with_capacity(items.len());
        for batch in batches {
            output.extend(batch.map_err(|error| -> Box<dyn std::error::Error> { error.into() })?);
        }
        Ok(output)
    })
}
