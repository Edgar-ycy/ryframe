/// 压缩中间件 layer 创建测试
use ryframe_middleware::compression_layer;

#[test]
fn test_compression_layer_creation() {
    // 测试 compression_layer 可以正常创建
    let _layer = compression_layer();
}
