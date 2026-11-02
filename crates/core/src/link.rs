//! §5.6.2 步骤 5 与安全规则那一格：码里读出来的那串字，**能不能进浏览器**、以及进之前要
//! display 给用户看的那一枚**完整域名**。
//!
//! 为什么这一层单独存在，而不是让 QML 自己 `indexOf("//")`：PRD 那句「打开外部链接前应显示
//! 完整域名」的全部价值就在于「显示的那一枚域名**就是**浏览器要去的那一枚」。窗上自己切第一
//! 个 `/`、自己切第一个 `@`，切出来的是 `paypal.com` 而浏览器去的是 `evil.com` —— 那正是二维码
//! 钓鱼的招牌形状（`https://paypal.com@evil.com/`），而一条按偏移量的切法**一定**会答错它。
//! 所以决定归这一层，窗只转达。
//!
//! 为什么不用 `url` crate：它确实更权威（完整的 WHATWG + IDNA），但它把 `idna` 连着整套
//! `icu_*`（normalizer / properties 的数据表）拖进闭包，为的是一枚我们只需要**显示**的域名。
//! 本模块只覆盖 http/https 那一支，并把这一支里与欺骗有关的规则**逐条点名**（下表），
//! 剩下的那些形态一律答「不打开」—— 宁可少开一扇门，不开一扇猜出来的门。
//!
//! 覆盖到的 WHATWG 规则（都在 [`classify`] 里，一条一格）：
//! - 解析**任何**部分之前先删掉整串里的 `\t` `\n` `\r`（否则 `https://ev\til.com` 切出来的
//!   host 与浏览器那个不是同一枚）；
//! - 首尾剥 C0 控制符与空格；
//! - scheme 必须真的存在且是 `http`/`https`（大小写无关）：没有 scheme 的裸主机名**不猜**，
//!   其它 scheme（`javascript:` `file:` `data:` `mailto:` `ftp:` …）一律不进浏览器；
//! - 对 special scheme，`\` 与 `/` 同权（`https://evil.com\@paypal.com` 的 host 是 `evil.com`，
//!   后面那半截是路径）；
//! - userinfo 按**最后一个** `@` 切（WHATWG 就是 `last @`），切掉的那半截不参与显示；
//! - 端口：非数字 / 0 / 越界都不算链接；等于该 scheme 默认口的端口**不显示**（与浏览器的
//!   序列化一致，`https://example.com:443/` 显示的仍是 `example.com`）；
//! - host 先 percent 解码，解码后仍含 `%` `@` `/` `\` `:` `[` `]` `?` `#` 空格的一律不打开
//!   （二次解码是这类解析器的经典后门）；
//! - IPv6 用方括号那一支单独走（`https://[::1]:8443/x` 的 host 是 `::1`）；
//! - host 含非 ASCII（IDN 同形字攻击的载体）**不打开**，并把原样那串交回给用户自己判断；
//!   `xn--` 那一支是纯 ASCII，因此照样打开且**按 punycode 原样显示** —— 把 `xn--pypa-…` 渲染成
//!   看着像 `paypal` 的那张字，正是这条规则要拦的事，不是它要做的事。

/// 一枚**可以**交给浏览器的 http(s) 链接。字段是给用户读的那三样，不是解析的中间产物。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Web {
    /// 真正交给 shell 的那一句：与送进解析器的那一句**只差**被删掉的控制字符与被剥掉的首尾
    /// 空白。不重新序列化 —— 重新拼一遍就多一套规则会和浏览器不一致，而这一格唯一的承诺是
    /// 「[`Web::host`] 是这一句里的主机名」。
    pub url: String,
    /// 完整域名：小写、已 percent 解码、不含 userinfo、不含路径、不含 scheme。
    pub host: String,
    /// 显式写了且不是默认口的那一枚端口。`None` = 用 scheme 的默认口，也就无需向用户额外说。
    pub port: Option<u16>,
    /// `true` = host 里有非 ASCII（IDN）。**这一枚不影响它是链接的判定**，影响的是开不开：
    /// 见模块头那条 IDN 规则。
    pub idn: bool,
}

impl Web {
    /// 打开之前摆给用户看的那一枚「完整域名」。端口只在显式且非默认时跟着一起说 ——
    /// `:8443` 换的是**另一台服务**，那是域名之外的一条信息，藏不得。
    pub fn display_host(&self) -> String {
        match self.port {
            Some(port) => format!("{}:{}", self.host, port),
            None => self.host.clone(),
        }
    }

    /// 这一枚能不能真开：IDN 答否。理由在模块头的 IDN 那条，一句话是不拿同形字替用户点头。
    pub fn openable(&self) -> bool {
        !self.idn
    }
}

/// [`classify`] 的答。四格各说一件事，是因为这四件事在界面上要说的话**不同**：把「不是链接」
/// 和「是链接但 scheme 不是浏览器那一类」挤进一格，界面就只能对两种情况说同一句话（#281）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// http(s) 且解析得出主机名。开不开再看 [`Web::openable`]。
    Web(Web),
    /// 有 scheme，但不是 `http`/`https`。带回来的是那一枚 scheme（已小写），界面要说的是
    /// 「这一枚不是浏览器链接」，而 `javascript` 与 `mailto` 值得让用户自己看见区别。
    Other(String),
    /// 没有 scheme、scheme 后面接不出主机名、或 host 过不了本模块那套字符表。
    NotALink,
}

const HTTP: &str = "http";
const HTTPS: &str = "https";

fn default_port(scheme: &str) -> u16 {
    match scheme {
        HTTP => 80,
        _ => 443,
    }
}

/// 这一串字是不是浏览器链接、是哪一枚域名。§5.6.2 步骤 5 与安全规则的那只手。
pub fn classify(text: &str) -> Target {
    // 规则一：控制字符在解析之前就没掉，而不是「跳过」——WHATWG 是删，不是绕。
    let scrubbed: String = text
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    // 规则二：首尾的 C0 与空格剥掉。用 `<= ' '` 而不是 `char::is_whitespace`：规范钉的是
    // C0 + U+0020 这一集合，而 NBSP 之类在 host 里是**内容**，不是包边。
    let trimmed = scrubbed.trim_matches(|c: char| c <= '\u{20}');

    let Some((scheme, rest)) = split_scheme(trimmed) else {
        return Target::NotALink;
    };
    if scheme != HTTP && scheme != HTTPS {
        return Target::Other(scheme);
    }
    // 规则三：special scheme 下 `\` 与 `/` 同权。整串替换是最省事且不引入分支的做法；路径里
    // 的 `\` 本来就会被浏览器当成 `/`，所以这一替换不会改变我们要开的那一句的目的地。
    let flat = rest.replace('\\', "/");
    let flat = flat.trim_start_matches('/');
    // 规则四：authority = 到第一个 `/` `?` `#` 为止的那一段。
    let authority = flat
        .split(['/', '?', '#'])
        .next()
        .expect("split always yields a first piece");
    // 规则五：userinfo 按最后一个 `@` 切。这一条是整个模块的存在理由：第一个 `@` 那一刀切出来
    // 的是攻击者想让用户看见的那半截。
    let hostport = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };

    let (host, port, idn) = if let Some(bracketed) = hostport.strip_prefix('[') {
        // IPv6 那一支：`]` 之前全是 host，之后只许是空或 `:端口`。这一支**不**走 percent 解码
        // 也不走域名那张字符表 —— 冒号是它的本行字符，用名字那套判它等于判死自己那条 IPv6 单测。
        let Some(idx) = bracketed.find(']') else {
            return Target::NotALink;
        };
        let inner = &bracketed[..idx];
        if !ip6_shaped(inner) {
            return Target::NotALink;
        }
        let tail = &bracketed[idx + 1..];
        let port = match tail.strip_prefix(':') {
            Some(text) => match parse_port(text, &scheme) {
                Some(port) => port,
                None => return Target::NotALink,
            },
            None if tail.is_empty() => None,
            // `[::1]x` 这种形态不是我们能替浏览器补的：它既不是端口，也不是路径的起点。
            None => return Target::NotALink,
        };
        (inner.to_ascii_lowercase(), port, false)
    } else {
        let (name, port) = match hostport.rsplit_once(':') {
            Some((host, text)) => match parse_port(text, &scheme) {
                Some(port) => (host, port),
                None => return Target::NotALink,
            },
            None => (hostport, None),
        };
        let Some(host) = decode_host(name) else {
            return Target::NotALink;
        };
        let idn_host = !host.is_ascii();
        (host, port, idn_host)
    };

    Target::Web(Web {
        url: trimmed.to_string(),
        host,
        port,
        idn,
    })
}

/// `None` = 这一串当不成一枚端口（非数字、0、越界、空）。`Some(None)` = 该 scheme 的默认口，
/// 也就是界面无需替它多说一句的那一格。
fn parse_port(text: &str, scheme: &str) -> Option<Option<u16>> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let parsed: u32 = text.parse().ok()?;
    if parsed == 0 || parsed > u32::from(u16::MAX) {
        return None;
    }
    let value = parsed as u16;
    Some((value != default_port(scheme)).then_some(value))
}

/// IPv6 字面量的那张表：十六进制位、冒号、点（IPv4 映射那一支要用），而且**不能带 `%`** ——
/// zone id（`%25eth0`）在浏览器地址栏那一支的去处不是这台机器的那张网卡，本模块不替它决定。
fn ip6_shaped(text: &str) -> bool {
    !text.is_empty()
        && text.contains(':')
        && text
            .bytes()
            .all(|b| b.is_ascii_hexdigit() || matches!(b, b':' | b'.'))
}

/// 切出（scheme 小写, 冒号之后那一截）。规则：首字符必须是 ASCII 字母，之后只许
/// ASCII 字母数字与 `+ - .`，并在遇到 `/` `?` `#` 之前撞上 `:`。撞不上就当没有 scheme ——
/// 裸主机名不当链接，因为「`example.com` 到底是不是链接」这一问没有确定答案，而猜的那一半
/// 落在**目的地**上。
fn split_scheme(text: &str) -> Option<(String, &str)> {
    let mut chars = text.char_indices();
    let (first, head) = chars.next()?;
    if !head.is_ascii_alphabetic() {
        return None;
    }
    for (idx, c) in chars {
        match c {
            ':' => return Some((text[first..idx].to_ascii_lowercase(), &text[idx + 1..])),
            '/' | '?' | '#' => return None,
            _ if c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.' => {}
            _ => return None,
        }
    }
    None
}

/// percent 解码 + 小写 + 剥掉那一枚可选的尾点（FQDN 的根标签，DNS 上与没有它等价，而浏览器
/// 的地址栏也不显示它）。解不动（非法 `%`、解出无效 UTF-8）就不是链接。
///
/// 解完之后仍出现的 `%` 一律判死：`%` 是这一层唯一的转义字符，留着它就等于允许
/// `%25` 把同一串字解两遍 —— 两次的答案不同，那正是「显示的域名与打开的域名」能分岔的地方。
fn decode_host(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = char::from(hex[0]).to_digit(16)?;
            let lo = char::from(hex[1]).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    let decoded = decoded.to_ascii_lowercase();
    // 解码不许带进来的字符：这些每一个都能把 host 的边界重新画一遍。
    if decoded
        .chars()
        .any(|c| matches!(c, '%' | '@' | '/' | '\\' | ':' | '[' | ']' | '?' | '#') || c <= '\u{20}')
    {
        return None;
    }
    let decoded = decoded.strip_suffix('.').unwrap_or(&decoded);
    // 空标签（`a..b`、`.a`）不是域名；它不影响目的地解析，但会影响**显示**：`a..b` 与 `a.b`
    // 在用户眼睛里几乎是同一串字，在 DNS 上不是。宁可判死。
    if decoded.is_empty() || decoded.split('.').any(|label| label.is_empty()) {
        return None;
    }
    // 规则六：host 的字符表。这里刻意**不**判标签长度、不判 TLD、不判 `_` 该不该出现在域名里
    // —— 那些是 DNS 的事，浏览器自己会拒；本表拦的是分隔符与控制符，也就是能改变目的地的字符。
    // 非 ASCII 是**放行**的（IDN 那一格由调用者判），其余一律 ASCII 白名单。
    if decoded
        .chars()
        .any(|c| c.is_ascii() && !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
    {
        return None;
    }
    Some(decoded.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web(text: &str) -> Web {
        match classify(text) {
            Target::Web(w) => w,
            other => panic!("expected a web link, got {other:?}"),
        }
    }

    #[test]
    fn an_plain_https_url_reports_its_host() {
        let got = web("https://falcon.example/pay?x=1#y");
        assert_eq!(got.host, "falcon.example");
        assert_eq!(got.port, None);
        assert!(got.openable());
        // 交回给 shell 的那一句是**原句**：查询、片段一个都不丢，否则就不是用户想要的那一页。
        assert_eq!(got.url, "https://falcon.example/pay?x=1#y");
    }

    /// 这一枚是本模块存在的理由：按第一个 `@` 切的写法在这里答 `paypal.com`，而浏览器去
    /// `evil.com`。显示必须与目的地同一枚。
    #[test]
    fn the_host_is_the_one_after_the_last_at() {
        let got = web("https://paypal.com@evil.com/confirm");
        assert_eq!(got.host, "evil.com", "userinfo must not pose as the domain");
        assert_eq!(got.display_host(), "evil.com");
    }

    /// special scheme 下 `\` 就是 `/`：`@` 落在路径里，userinfo 一格也没有。
    #[test]
    fn a_backslash_ends_the_authority() {
        let got = web("https://evil.com\\@paypal.com/x");
        assert_eq!(got.host, "evil.com");
    }

    #[test]
    fn tab_and_newline_are_gone_before_anything_is_parsed() {
        assert_eq!(web("https://ev\til.com/").host, "evil.com");
        assert_eq!(web("  http://a.example/\r\n").host, "a.example");
    }

    #[test]
    fn scheme_and_case_are_normalized() {
        let got = web("HTTPS://Example.COM/Path");
        assert_eq!(got.host, "example.com");
        assert_eq!(got.url, "HTTPS://Example.COM/Path", "目的地那句不改写");
    }

    #[test]
    fn a_non_default_port_is_displayed_and_a_default_one_is_not() {
        assert_eq!(
            web("https://a.example:8443/").display_host(),
            "a.example:8443"
        );
        assert_eq!(web("https://a.example:443/").display_host(), "a.example");
        assert_eq!(web("http://a.example:80/").display_host(), "a.example");
        assert_eq!(web("http://a.example:81/").display_host(), "a.example:81");
    }

    #[test]
    fn a_host_can_be_an_ipv6_loopback() {
        let got = web("https://[::1]:8443/x");
        assert_eq!(got.host, "::1");
        assert_eq!(got.port, Some(8443));
    }

    #[test]
    fn percent_escapes_are_decoded_once_and_never_re_encoded() {
        assert_eq!(web("https://ex%61mple.com/").host, "example.com");
        // 解完还剩 `%2f`：等于允许再解一遍，而第二遍的边界不一样。
        assert_eq!(classify("https://a%252fb.example/"), Target::NotALink);
        assert_eq!(classify("https://exa%nmple.com/"), Target::NotALink);
    }

    #[test]
    fn other_schemes_never_reach_a_browser() {
        for (text, scheme) in [
            ("mailto:someone@example.com", "mailto"),
            ("javascript:alert(1)", "javascript"),
            ("file:///C:/Windows/system32/cmd.exe", "file"),
            ("FTP://example.com/a", "ftp"),
            ("data:text/html,<script>alert(1)</script>", "data"),
        ] {
            assert_eq!(
                classify(text),
                Target::Other(scheme.to_string()),
                "wrong for {text}"
            );
        }
    }

    #[test]
    fn a_bare_hostname_is_not_a_link() {
        assert_eq!(classify("example.com/pay"), Target::NotALink);
        assert_eq!(classify("www.example.com"), Target::NotALink);
        assert_eq!(classify(""), Target::NotALink);
        assert_eq!(classify("   "), Target::NotALink);
    }

    #[test]
    fn a_host_that_cannot_be_a_host_is_not_a_link() {
        for text in [
            "https://",
            "https://exa mple.com/",
            "https://a.example:8x/",
            "https://a.example:0/",
            "https://a.example:99999999/",
            "https://a..example/",
            "https://.example/",
            "https://a.example:/x",
            "http:/",
        ] {
            assert_eq!(classify(text), Target::NotALink, "wrong for {text}");
        }
    }

    /// `https:///x` 看着像病句，但 WHATWG 的 special-scheme 那一支会把 `://` 之后**所有**的斜杠
    /// 一起吃掉再进 host state，浏览器因此把 `x` 当主机名（Chrome 会去解 `x` 这个名字）。本模块
    /// 的答案必须与它一致 —— 判它 `NotALink` 才是错的那一方，因为「显示 = 目的地」这一条承诺
    /// 要对着的是浏览器实际会去的那一枚主机，不是我们觉得它**应该**是什么。
    #[test]
    fn extra_slashes_collapse_the_way_the_browser_collapses_them() {
        assert_eq!(web("https:///x").host, "x");
        assert_eq!(web("http:example.com/x").host, "example.com");
        assert_eq!(web("https://\\\\a.example/").host, "a.example");
    }

    /// 同形字那一支：不是链接吗？是。能开吗？不能。这两格分开写，是因为界面要说的是
    /// 「这一枚是 IDN，本轮不开」而不是骗用户「这不是链接」。
    #[test]
    fn an_idn_host_is_a_link_we_do_not_open() {
        let got = web("https://exämple.com/");
        assert_eq!(got.host, "exämple.com");
        assert!(got.idn);
        assert!(!got.openable());
    }

    /// punycode 是纯 ASCII，所以它照样能开 —— 而显示的是这一串，不是被谁「美化」过的那一串。
    #[test]
    fn punycode_is_displayed_as_punycode() {
        let got = web("https://xn--p1ai.example/");
        assert_eq!(got.host, "xn--p1ai.example");
        assert!(!got.idn);
        assert!(got.openable());
    }

    /// `http:example.com` 这类「special scheme 后面不写双斜杠」浏览器是接受的，我们的解析器
    /// 也接受；不接受的只有既没 scheme 也没主机的形态。
    #[test]
    fn slashes_after_the_scheme_are_optional() {
        assert_eq!(web("http:example.com/x").host, "example.com");
        assert_eq!(web("https://example.com").host, "example.com");
    }

    /// 二维码里带前后空白是家常便饭（生成器爱加 `\n`）。这一枚保证空白不会被算进 host。
    #[test]
    fn surrounding_whitespace_is_not_part_of_anything() {
        let got = web(" https://a.example/path \n");
        assert_eq!(got.host, "a.example");
        assert_eq!(got.url, "https://a.example/path");
    }
}
