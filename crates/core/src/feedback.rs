//! PRD §5.21.3「问题反馈」：反馈包能带什么、以及哪些东西不许跟着它走。
//!
//! 两条隐私规则决定了这一档的形状：
//! - 「未经用户确认不得上传截图内容」⇒ [`Draft`] 一个字节字段也没有：进得来包的只有调用方已经
//!   拿在手里的文本，所以一张图**没法**被夹带进来 —— 这一条在类型上就成立，不必等到出口再查。
//! - 「日志中应移除密码、令牌和其他敏感信息」⇒ 机器生成的每一行都得过 [`redact_line`]。
//!   用户自己写的那句描述按 PRD 逐字带走（规则写的是「日志中」，不是「用户的话里」），
//!   这一点是择一实现并记名，不是把冲突解掉了。

/// 值不许外带的键名（小写；比对时也把原文折成小写）。点号分段的配置写法按段查，
/// 所以 `app.password=x` 与 `password=x` 同判，而 `about.send_diagnostics=false` 两段的都不在表上。
pub const SENSITIVE_KEYS: &[&str] = &[
    "access_key",
    "access_token",
    "apikey",
    "api_key",
    "auth",
    "authorization",
    "cookie",
    "credential",
    "license",
    "passwd",
    "password",
    "private_key",
    "pwd",
    "refresh_token",
    "secret",
    "session",
    "session_id",
    "sessionid",
    "set-cookie",
    "token",
    "webhook",
];

/// 替掉值的那一枚记号。它本身不算敏感值，所以第二次扫这一行时不会重复计数。
pub const MARK: &str = "[redacted]";

/// 一行的脱敏结果：改完的文本，与改掉的枚数。
pub struct Redacted {
    pub text: String,
    pub hits: usize,
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

/// 值的终点：空白与这几枚分隔符。逐颗走到这里就停，因此不会把一个多字节汉字切一半。
fn is_value_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, ',' | ';' | '&' | '"' | '\'' | '}' | ']')
}

fn is_sensitive_key(run: &str) -> bool {
    run.split('.').any(|part| SENSITIVE_KEYS.contains(&part))
}

/// 一行里所有 `键 = 值` / `键: 值` / JSON `"键": "值"` 形式的值，只要键名在表上就换掉。
/// 值以 `Bearer` / `Basic` 开头时那一枚令牌一并带走，否则 `Authorization: Bearer <tok>` 会只藏住
/// 前半截 —— 计数仍然算一枚。
pub fn redact_line(line: &str) -> Redacted {
    let cs: Vec<char> = line.chars().collect();
    let low: String = cs.iter().map(|c| c.to_ascii_lowercase()).collect();
    let lc: Vec<char> = low.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut hits = 0usize;
    let mut i = 0usize;
    while i < cs.len() {
        if lc[i] == '=' || lc[i] == ':' {
            // 往回取键名那一串：跳过空白与一枚收尾的引号。
            let mut j = i;
            while j > 0 && (lc[j - 1] == ' ' || lc[j - 1] == '\t' || lc[j - 1] == '"') {
                j -= 1;
            }
            let e = j;
            while j > 0 && is_word(lc[j - 1]) {
                j -= 1;
            }
            let key: String = lc[j..e].iter().collect();
            if !key.is_empty() && is_sensitive_key(&key) {
                out.push(cs[i]);
                i += 1;
                while i < cs.len() && (lc[i] == ' ' || lc[i] == '\t' || lc[i] == '"') {
                    out.push(cs[i]);
                    i += 1;
                }
                let start = i;
                while i < cs.len() && !is_value_end(cs[i]) {
                    i += 1;
                }
                let val: String = lc[start..i].iter().collect();
                out.push_str(MARK);
                hits += 1;
                if val == "bearer" || val == "basic" {
                    // 令牌是空格后面的那一串：一起藏，才算藏住这一枚。
                    let mut k = i;
                    while k < cs.len() && (cs[k] == ' ' || cs[k] == '\t') {
                        k += 1;
                    }
                    let t0 = k;
                    while k < cs.len() && !is_value_end(cs[k]) {
                        k += 1;
                    }
                    if k > t0 {
                        out.extend(cs[i..t0].iter());
                        out.push_str(MARK);
                        i = k;
                    }
                }
                continue;
            }
        }
        // 主目录路径：`C:\Users\alice\…` 里那一个名字不带出去。
        if lc[i] == '\\'
            && i + 7 <= lc.len()
            && lc[i..i + 7].iter().collect::<String>() == "\\users\\"
        {
            out.extend(cs[i..i + 7].iter());
            i += 7;
            let mut k = i;
            while k < cs.len() && cs[k] != '\\' && cs[k] != '/' && !cs[k].is_whitespace() {
                k += 1;
            }
            if k > i {
                out.push_str(MARK);
                hits += 1;
                i = k;
            }
            continue;
        }
        out.push(cs[i]);
        i += 1;
    }
    Redacted { text: out, hits }
}

/// 一份反馈的草稿。三个字段全是文本：截图内容进不来（PRD §5.21.3 隐私规则第一条）。
#[derive(Debug, Default)]
pub struct Draft {
    /// 用户自己写的那句话（步骤 3），逐字带走。
    pub description: String,
    /// 机器生成的诊断行（步骤 2）。
    pub diagnostics: Vec<String>,
    /// 日志行（步骤 3 里用户「决定是否附加」的那一枚）。
    pub logs: Vec<String>,
}

/// 可以发出去的包。`attached=false` 时机器那一半整批留在原地，只有用户那句话进包。
#[derive(Debug)]
pub struct Payload {
    pub lines: Vec<String>,
    /// 包里被藏掉的枚数（诊断行与日志行合计）。
    pub redactions: usize,
    pub attached: bool,
}

/// 组装。两把闸都在才带机器那一半：配置里那一枚 `about.send_diagnostics`，与用户这一次点下头。
/// 少任何一把，诊断与日志一枚也不进包 —— 而包里的文本已经先过了一遍 [`redact_line`]。
pub fn payload(
    draft: &Draft,
    send_diagnostics: bool,
    user_confirmed: bool,
    version: &str,
    os: &str,
) -> Payload {
    let attached = send_diagnostics && user_confirmed;
    let mut lines = vec![
        format!("feedback v={version} os={os}"),
        format!("desc={}", draft.description),
    ];
    let mut redactions = 0usize;
    if attached {
        for d in &draft.diagnostics {
            let r = redact_line(d);
            redactions += r.hits;
            lines.push(format!("diag={}", r.text));
        }
        for l in &draft.logs {
            let r = redact_line(l);
            redactions += r.hits;
            lines.push(format!("log={}", r.text));
        }
    } else {
        lines.push(format!(
            "machine=omitted send_diagnostics={send_diagnostics} confirmed={user_confirmed}"
        ));
    }
    Payload {
        lines,
        redactions,
        attached,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(line: &str) -> (String, usize) {
        let r = redact_line(line);
        (r.text, r.hits)
    }

    #[test]
    fn a_password_value_is_replaced_not_dropped() {
        assert_eq!(
            one("login password=hunter2 ok"),
            ("login password=[redacted] ok".to_string(), 1)
        );
    }

    #[test]
    fn a_dotted_config_key_is_read_by_its_last_part() {
        assert_eq!(
            one("app.password = s3cret"),
            ("app.password = [redacted]".to_string(), 1)
        );
    }

    #[test]
    fn a_json_pair_is_scrubbed_inside_its_quotes() {
        assert_eq!(
            one(r#"{"token": "eyJhbGci","n":3}"#),
            (r#"{"token": "[redacted]","n":3}"#.to_string(), 1)
        );
    }

    #[test]
    fn an_authorization_header_loses_its_token_too() {
        let (text, hits) = one("Authorization: Bearer eyJhbGciOiJIUz12");
        assert_eq!(text, "Authorization: [redacted] [redacted]");
        assert_eq!(hits, 1);
    }

    #[test]
    fn a_query_string_stops_at_the_next_parameter() {
        assert_eq!(
            one("https://h/x?token=abc&size=3"),
            ("https://h/x?token=[redacted]&size=3".to_string(), 1)
        );
    }

    #[test]
    fn two_secrets_on_one_line_are_counted_twice() {
        let (text, hits) = one("token=a secret=b");
        assert_eq!(text, "token=[redacted] secret=[redacted]");
        assert_eq!(hits, 2);
    }

    #[test]
    fn a_key_that_merely_starts_like_a_secret_keeps_its_value() {
        for line in [
            "tokenized=1",
            "sessions=3",
            "passwdfile=x",
            "about.send_diagnostics=false",
            "history.max_items=50",
        ] {
            let (text, hits) = one(line);
            assert_eq!((text.as_str(), hits), (line, 0), "{line} was rewritten");
        }
    }

    #[test]
    fn a_home_directory_loses_its_user_name() {
        let (text, hits) = one(r#"file=C:\Users\alice\Desktop\a.png"#);
        assert_eq!(text, r#"file=C:\Users\[redacted]\Desktop\a.png"#);
        assert_eq!(hits, 1);
    }

    #[test]
    fn chinese_text_survives_a_redaction() {
        let (text, hits) = one("贴图没出现 token=abc 第二屏");
        assert_eq!(text, "贴图没出现 token=[redacted] 第二屏");
        assert_eq!(hits, 1);
    }

    #[test]
    fn the_machine_half_never_travels_without_a_confirmation() {
        let d = Draft {
            description: "打不开".into(),
            diagnostics: vec!["password=x".into()],
            logs: vec!["token=y".into()],
        };
        let p = payload(&d, true, false, "1.0", "win");
        assert!(!p.attached);
        assert_eq!(p.lines.len(), 3);
        assert_eq!(p.redactions, 0);
        assert!(p.lines[2].starts_with("machine=omitted"));
        assert!(!p.lines.iter().any(|l| l.contains("password")));
        assert!(!p.lines.iter().any(|l| l.contains("token")));

        let q = payload(&d, false, true, "1.0", "win");
        assert!(!q.attached, "the config's own switch is a gate too");
    }

    #[test]
    fn both_gates_open_carry_scrubbed_lines() {
        let d = Draft {
            description: "打不开".into(),
            diagnostics: vec!["password=x".into()],
            logs: vec!["run 1 token=y".into()],
        };
        let p = payload(&d, true, true, "1.0", "win");
        assert!(p.attached);
        assert_eq!(p.redactions, 2);
        assert_eq!(p.lines[2], "diag=password=[redacted]");
        assert_eq!(p.lines[3], "log=run 1 token=[redacted]");
    }

    #[test]
    fn the_users_own_sentence_travels_unchanged() {
        let d = Draft {
            description: "password=hunter2 就是我看到的".into(),
            diagnostics: vec!["password=hunter2".into()],
            logs: vec![],
        };
        let p = payload(&d, true, true, "1.0", "win");
        assert_eq!(p.lines[1], "desc=password=hunter2 就是我看到的");
        assert_eq!(p.lines[2], "diag=password=[redacted]");
    }

    #[test]
    fn a_bundle_is_text_only_however_big_the_picture_is() {
        // PRD 第一条隐私规则的这一半是可数的：一张 4,000,000 颗像素的图，包里出现的只有路径。
        let (w, h) = (2000u32, 2000u32);
        let d = Draft {
            description: "贴图".into(),
            diagnostics: vec![format!("saved path=C:\\Users\\bob\\p.png w={w} h={h}")],
            logs: vec![],
        };
        let p = payload(&d, true, true, "1.0", "win");
        let bytes: usize = p.lines.iter().map(|l| l.len()).sum();
        assert_eq!(w * h, 4_000_000);
        assert!(
            bytes < 300,
            "the bundle carried {bytes} B for one {} MP picture",
            w * h / 1_000_000
        );
        assert!(p.lines[2].contains("p.png"), "path stays: {}", p.lines[2]);
    }
}
