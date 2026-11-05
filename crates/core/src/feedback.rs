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

/// 可以发出去的包。机器那两半各有各的闸（见 [`payload`]），所以这里把两半分开记：合在一条
/// `attached` 上，「只准带诊断、日志没勾」与「一枚机器字也不带」就成了同一个读数。
#[derive(Debug)]
pub struct Payload {
    pub lines: Vec<String>,
    /// 包里被藏掉的枚数（诊断行与日志行合计）。
    pub redactions: usize,
    /// 诊断那一半出去了没有。
    pub diagnostics: bool,
    /// 日志那一半出去了没有。
    pub logs: bool,
}

impl Payload {
    /// 机器那一半这一趟有没有被允许出去（允许了而没行可带，包里有的是那一句 `…=none rows=0`，
    /// 见 [`payload`]）—— 两把闸都关时包里有且只有用户那句话。
    ///
    /// 它不是第三枚事实：它是 [`Self::diagnostics`] 与 [`Self::logs`] 的「或」，写成方法是为了让
    /// 「省略那一行该不该出现」只有一处问法，而不是每个读者自己 `||` 一遍（#281）。
    pub fn attached(&self) -> bool {
        self.diagnostics || self.logs
    }
}

/// 用户在这一趟里点下的两样。**它们是两个答案，不是一枚开关的两档**：日志那一半由 §5.21.3
/// 第 3 步那枚「是否附加」的勾选框决定，诊断那一半由配置里那枚开关＋这一次点下头决定。
#[derive(Debug, Clone, Copy)]
pub struct Consent {
    /// 按下「发送反馈」了没有。没点头，机器那两半一枚也不走 —— 隐私规则第一条管的就是这一格。
    pub confirmed: bool,
    /// 「附加日志」那一枚勾选框此刻的样子。
    pub logs: bool,
}

/// 组装。诊断与日志各有各的闸（见 [`Consent`]）：
/// - 诊断：配置里那枚 `about.send_diagnostics` **与**用户这一次点下头，两把都在才带；
/// - 日志：只有用户自己那枚勾选框**与**点下头。配置那一枚开关管的是诊断信息（§5.16 那一枚
///   「允许收集诊断信息」），它不该替用户决定日志给不给 —— 拿它去闸日志，用户看到的是一句
///   「我勾了，却没带」，而这一档说不出为什么。
///
/// 少任何一把，那一半一枚也不进包 —— 而进了包的文本已经先过了一遍 [`redact_line`]。
/// 两把都在而那一半一行也没有，包里有的是那一句 `…=none rows=0`：省略句说的是**谁关掉的**，
/// 这里没人关掉任何东西，两句话不许顶替彼此。
pub fn payload(
    draft: &Draft,
    send_diagnostics: bool,
    consent: &Consent,
    version: &str,
    os: &str,
) -> Payload {
    let diagnostics = send_diagnostics && consent.confirmed;
    let logs = consent.logs && consent.confirmed;
    let mut lines = vec![
        format!("feedback v={version} os={os}"),
        format!("desc={}", draft.description),
    ];
    let mut redactions = 0usize;
    for (on, rows, label) in [
        (diagnostics, &draft.diagnostics, "diag"),
        (logs, &draft.logs, "log"),
    ] {
        if on {
            if rows.is_empty() {
                // 闸开了而这一趟一行也没有，那一行也得说：省略那一句管的是「谁把它关掉了」，
                // 而这里没人关它 —— 把两件事写成同一句，收件的那一位就再也分不出「用户没让带」
                // 与「用户让带而这台机器没什么可带」（#453）。
                lines.push(format!("{label}=none rows=0"));
            } else {
                for d in rows {
                    let r = redact_line(d);
                    redactions += r.hits;
                    lines.push(format!("{label}={}", r.text));
                }
            }
        } else {
            // 省略的那一行说的是**哪一半**被谁关掉的：「配置那枚开关没开」与「用户没勾」与
            // 「用户压根没点发送」是三件事，合成一句「machine=omitted」就再也分不开（#453）。
            lines.push(format!(
                "{label}=omitted send_diagnostics={send_diagnostics} logs_box={} confirmed={}",
                consent.logs, consent.confirmed
            ));
        }
    }
    Payload {
        lines,
        redactions,
        diagnostics,
        logs,
    }
}

impl Payload {
    /// 这一份包落成一段文本时的样子：`lines` 逐行照抄、行尾各补一枚换行。
    ///
    /// 它是「包写成文本」这枚事实唯一的作者。谁要把包写进文件、贴进剪贴板、或交给任何一段
    /// 文本通道，都从这里取，不在自己那儿再把 `lines` 拼一遍 —— 更不在自己那儿再过一遍
    /// [`redact_line`]：那一道已经在 [`payload`] 里逐行做过了，再来一遍只会把「谁藏了几枚」
    /// 这件事变成两处各说各话（#281）。
    pub fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
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
        // 用户没点发送，而日志那枚勾是勾着的：那一半也一枚不走，因为它闸的是「出去」这一步。
        let unasked = payload(
            &d,
            true,
            &Consent {
                confirmed: false,
                logs: true,
            },
            "1.0",
            "win",
        );
        assert!(!unasked.attached());
        assert_eq!(unasked.lines.len(), 4, "{:?}", unasked.lines);
        assert_eq!(unasked.redactions, 0);
        assert!(unasked.lines[2].starts_with("diag=omitted"));
        assert!(unasked.lines[3].starts_with("log=omitted"));
        assert!(!unasked.lines.iter().any(|l| l.contains("password")));
        assert!(!unasked.lines.iter().any(|l| l.contains("token")));

        let gated = payload(
            &d,
            false,
            &Consent {
                confirmed: true,
                logs: true,
            },
            "1.0",
            "win",
        );
        assert!(!gated.diagnostics, "配置里那枚开关诊断那一半自己说了不算");
        assert!(gated.logs, "配置那枚开关不该替用户把日志也关掉");
    }

    #[test]
    fn the_logs_box_alone_decides_the_logs() {
        // §5.21.3 第 3 步「决定是否附加日志」在页上是一枚勾选框，而它决定的只有日志那一半：
        // 没勾 ⇒ 日志两行一枚不进包，诊断照旧跟着配置那枚开关走。两半合用一条 attached 就读不出
        // 这一条，所以这一格判的是两枚字段各红各的。
        let d = Draft {
            description: "打不开".into(),
            diagnostics: vec!["password=x".into()],
            logs: vec!["token=y".into(), "run 2 secret=z".into()],
        };
        let off = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: false,
            },
            "1.0",
            "win",
        );
        assert!(off.diagnostics && !off.logs);
        assert_eq!(off.lines.len(), 4);
        assert_eq!(off.lines[2], "diag=password=[redacted]");
        assert!(off.lines[3].starts_with("log=omitted"));
        assert!(
            off.lines[3].contains("logs_box=false"),
            "省略的那一行要说清是哪一枚勾关掉的：{}",
            off.lines[3]
        );
        assert!(!off.lines.iter().any(|l| l.contains("token")));

        let on = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: true,
            },
            "1.0",
            "win",
        );
        assert!(on.diagnostics && on.logs);
        assert_eq!(on.lines.len(), 5);
        assert_eq!(on.redactions, 3, "诊断一枚、日志两枚");
        assert_eq!(on.lines[4], "log=run 2 secret=[redacted]");
    }

    #[test]
    fn a_consent_with_nothing_to_carry_says_so() {
        // 两把闸都开了而这台机器一行也没生成出来：那一半在包里占一行 `…=none rows=0`，不是
        // 省略句 —— 「用户没让带」与「用户让带而没东西可带」是两件事，收件的那一位只能从字上分。
        let d = Draft {
            description: "打不开".into(),
            diagnostics: vec![],
            logs: vec![],
        };
        let on = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: true,
            },
            "1.0",
            "win",
        );
        let off = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: false,
            },
            "1.0",
            "win",
        );
        assert_eq!(on.lines[2], "diag=none rows=0");
        assert_eq!(on.lines[3], "log=none rows=0");
        assert_eq!(off.lines[2], "diag=none rows=0", "诊断那一半不归这枚勾管");
        assert!(off.lines[3].starts_with("log=omitted"));
        assert_eq!(on.redactions, 0);
        assert!(on.attached(), "两半都是开着的那一支，行数零不零是另一件事");
    }

    #[test]
    fn both_gates_open_carry_scrubbed_lines() {
        let d = Draft {
            description: "打不开".into(),
            diagnostics: vec!["password=x".into()],
            logs: vec!["run 1 token=y".into()],
        };
        let p = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: true,
            },
            "1.0",
            "win",
        );
        assert!(p.attached());
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
        let p = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: true,
            },
            "1.0",
            "win",
        );
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
        let p = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: true,
            },
            "1.0",
            "win",
        );
        let bytes: usize = p.lines.iter().map(|l| l.len()).sum();
        assert_eq!(w * h, 4_000_000);
        assert!(
            bytes < 400,
            "the bundle carried {bytes} B for one {} MP picture",
            w * h / 1_000_000
        );
        assert!(p.lines[2].contains("p.png"), "path stays: {}", p.lines[2]);
    }

    #[test]
    fn the_written_text_scrubs_every_sensitive_key_it_is_handed() {
        // 第二条隐私规则的「日志中应移除」在这里是可数的：把 SENSITIVE_KEYS 整表各喂一行进
        // 日志，落盘文本里一枚原值也不留，而枚数与 [`Payload::redactions`] 对得上。
        let logs: Vec<String> = SENSITIVE_KEYS
            .iter()
            .map(|k| format!("run 1043 {k}=LEAKVALUE tail"))
            .collect();
        let n = logs.len();
        let d = Draft {
            description: "点了没反应".into(),
            diagnostics: vec!["ai.api_key = LEAKKEY".into()],
            logs,
        };
        let p = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: true,
            },
            "1.0",
            "win",
        );
        assert_eq!(p.redactions, n + 1, "每一枚键各一枚，加诊断那一行一枚");
        let text = p.text();
        assert!(!text.contains("LEAKVALUE"), "原值漏出去了：{text}");
        assert!(!text.contains("LEAKKEY"), "诊断的原值漏出去了：{text}");
        assert_eq!(text.matches(MARK).count(), n + 1);
        // text() 只是把 lines 逐行照抄：不多写一行，也不少写一行。
        assert_eq!(text.lines().count(), p.lines.len());
        assert!(text.ends_with('\n'), "落盘的东西末尾得有一枚换行");
    }

    #[test]
    fn text_does_not_redact_a_second_time() {
        // #281：〔谁被藏了〕只有 payload 一处说话。用户自己那句按 PRD 逐字带走，
        // 所以 text() 里那句 desc 若含 `password=`，它留原值、且 redactions 不因它而变。
        let d = Draft {
            description: "password=hunter2 就是我看到的".into(),
            diagnostics: vec![],
            logs: vec![],
        };
        let p = payload(
            &d,
            true,
            &Consent {
                confirmed: true,
                logs: true,
            },
            "1.0",
            "win",
        );
        assert_eq!(p.redactions, 0);
        assert!(p.text().contains("desc=password=hunter2 就是我看到的"));
    }
}
