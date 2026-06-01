//! 邮件通知服务
//!
//! 基于 `lettre` crate 的 SMTP 邮件发送封装。
//! 支持 TLS/STARTTLS，HTML 和纯文本邮件。

use lettre::{
    AsyncTransport, Message,
    message::{Mailbox, header},
    transport::smtp::{
        AsyncSmtpTransport,
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
};

/// 邮件配置
#[derive(Debug, Clone)]
pub struct EmailConfig {
    /// SMTP 服务器地址
    pub smtp_host: String,
    /// SMTP 端口（465=SSL, 587=STARTTLS）
    pub smtp_port: u16,
    /// SMTP 用户名
    pub smtp_username: String,
    /// SMTP 密码
    pub smtp_password: String,
    /// 发件人名称
    pub from_name: String,
    /// 发件人邮箱
    pub from_email: String,
    /// 是否启用 TLS
    pub enable_tls: bool,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            smtp_host: "smtp.example.com".into(),
            smtp_port: 587,
            smtp_username: String::new(),
            smtp_password: String::new(),
            from_name: "System".into(),
            from_email: "noreply@example.com".into(),
            enable_tls: true,
        }
    }
}

/// 邮件发送器
#[derive(Clone)]
pub struct EmailSender {
    config: EmailConfig,
}

impl EmailSender {
    /// 创建邮件发送器
    pub fn new(config: EmailConfig) -> Self {
        Self { config }
    }

    /// 构建 SMTP 传输器
    fn build_transport(&self) -> Result<AsyncSmtpTransport<lettre::Tokio1Executor>, String> {
        let tls = if self.config.enable_tls {
            Tls::Opportunistic(
                TlsParameters::builder(self.config.smtp_host.clone())
                    .build_rustls()
                    .map_err(|e| format!("TLS 配置失败: {}", e))?,
            )
        } else {
            Tls::None
        };

        let creds = Credentials::new(
            self.config.smtp_username.clone(),
            self.config.smtp_password.clone(),
        );

        Ok(
            AsyncSmtpTransport::<lettre::Tokio1Executor>::relay(&self.config.smtp_host)
                .map_err(|e| format!("SMTP relay 配置失败: {}", e))?
                .port(self.config.smtp_port)
                .credentials(creds)
                .tls(tls)
                .build(),
        )
    }

    /// 构建邮件消息
    pub fn build_message(
        &self,
        to: &str,
        subject: &str,
        html_body: &str,
    ) -> Result<Message, String> {
        let from: Mailbox = format!("{} <{}>", self.config.from_name, self.config.from_email)
            .parse()
            .map_err(|e| format!("发件人格式错误: {}", e))?;

        let to: Mailbox = to.parse().map_err(|e| format!("收件人格式错误: {}", e))?;

        Message::builder()
            .from(from)
            .to(to)
            .subject(subject)
            .header(header::ContentType::TEXT_HTML)
            .body(html_body.to_string())
            .map_err(|e| format!("邮件构建失败: {}", e))
    }

    /// 发送 HTML 邮件
    ///
    /// # 示例
    /// ```
    /// use ryframe_common::utils::email::{EmailSender, EmailConfig};
    ///
    /// let config = EmailConfig::default();
    /// let sender = EmailSender::new(config);
    /// // sender.send("user@example.com", "验证码", "<h1>123456</h1>").await?;
    /// ```
    pub async fn send(&self, to: &str, subject: &str, html_body: &str) -> Result<(), String> {
        let message = self.build_message(to, subject, html_body)?;
        let transport = self.build_transport()?;

        transport
            .send(message)
            .await
            .map_err(|e| format!("邮件发送失败: {}", e))?;

        tracing::info!("邮件已发送: to={}, subject={}", to, subject);
        Ok(())
    }

    /// 发送纯文本邮件
    pub async fn send_text(&self, to: &str, subject: &str, body: &str) -> Result<(), String> {
        let from: Mailbox = format!("{} <{}>", self.config.from_name, self.config.from_email)
            .parse()
            .map_err(|e| format!("发件人格式错误: {}", e))?;

        let to: Mailbox = to.parse().map_err(|e| format!("收件人格式错误: {}", e))?;

        let message = Message::builder()
            .from(from)
            .to(to)
            .subject(subject)
            .header(header::ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| format!("邮件构建失败: {}", e))?;

        let transport = self.build_transport()?;
        transport
            .send(message)
            .await
            .map_err(|e| format!("邮件发送失败: {}", e))?;

        Ok(())
    }

    /// 发送验证码邮件
    pub async fn send_verification_code(
        &self,
        to: &str,
        code: &str,
        expire_minutes: u32,
    ) -> Result<(), String> {
        let html = format!(
            r#"<!DOCTYPE html>
<html>
<body style="font-family: Arial, sans-serif; max-width: 600px; margin: 0 auto;">
    <div style="background: #4F46E5; padding: 20px; text-align: center;">
        <h1 style="color: white; margin: 0;">验证码</h1>
    </div>
    <div style="padding: 30px; background: #f9fafb;">
        <p>您好，</p>
        <p>您的验证码是：</p>
        <div style="background: white; border: 2px dashed #4F46E5; border-radius: 8px; 
                    padding: 20px; text-align: center; margin: 20px 0;">
            <span style="font-size: 32px; font-weight: bold; color: #4F46E5; 
                         letter-spacing: 8px;">{}</span>
        </div>
        <p>验证码有效期 <strong>{} 分钟</strong>，请勿泄露给他人。</p>
        <hr style="border: none; border-top: 1px solid #e5e7eb; margin: 20px 0;">
        <p style="color: #6b7280; font-size: 12px;">
            此邮件由系统自动发送，请勿回复。
        </p>
    </div>
</body>
</html>"#,
            code, expire_minutes
        );

        self.send(to, "验证码", &html).await
    }
}
