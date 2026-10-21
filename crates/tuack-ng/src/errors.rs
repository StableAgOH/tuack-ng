//! 错误分类与输出约定。
//!
//! 错误分类：
//!
//! - **用户问题**：在抛出点就地输出，然后返回 [`Reported`] 作为「已报告」状态。顶层只据此
//!   落成退出码 1，不再输出任何东西。
//! - **自身问题**（例外）：任何**不是** [`Reported`] 的错误都视为未处理的自身问题——Tuack-NG
//!   的 Bug、安装不正确，也包括某处用户问题忘了就地报告。由顶层输出并以退出码 2 退出。
//!
//! 因此规则很简单：用户问题必须带 [`Reported`] 上抛；不带标记的错误一律按自身问题处理。

/// 用户问题：已经在抛出点输出，退出码 1
///
/// 只表示「已报告」，不携带消息。
#[derive(Debug)]
pub struct Reported;

impl std::fmt::Display for Reported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 顶层不会输出它；保留实现以满足 `std::error::Error`
        f.write_str("遇到了用户有关的问题")
    }
}

impl std::error::Error for Reported {}

/// 错误链里是否已有「已报告」标记
pub fn is_reported(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Reported>().is_some()
}

/// 输出用户有关的问题，并返回「已报告」状态
///
/// 带参数时输出消息，不带参数时不输出（细节已在别处打印）。
#[macro_export]
macro_rules! user_error {
    () => {
        $crate::errors::Reported
    };
    ($($arg:tt)*) => {
        {
            $crate::msg_error!("{}", format!($($arg)*));
            $crate::errors::Reported
        }
    };
}

/// 输出用户有关的问题并返回（`return Err(...)`）
///
/// 带参数时先输出再返回；不带参数时只返回「已报告」状态。
/// 用户问题必须经由本宏（或 [`user_error!`]）上抛，否则会被顶层当成自身问题。
#[macro_export]
macro_rules! user_bail {
    () => {
        return ::core::result::Result::Err(::core::convert::Into::into(
            $crate::errors::Reported,
        ));
    };
    ($($arg:tt)*) => {
        {
            return ::core::result::Result::Err(::core::convert::Into::into(
                $crate::user_error!($($arg)*),
            ));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_reported() {
        assert!(is_reported(&anyhow::Error::new(Reported)));
        assert!(is_reported(
            &anyhow::Error::new(Reported).context("外层上下文")
        ));
    }

    #[test]
    fn plain_error_is_not_reported() {
        assert!(!is_reported(&anyhow::anyhow!("boom")));
        assert!(!is_reported(&anyhow::anyhow!("boom").context("外层上下文")));
    }
}
