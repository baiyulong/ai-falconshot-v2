//! §5.5.10 发送到其他应用: the one decision step 3 makes, and the sentence 规则 2 asks for.
//!
//! The requirement is three steps and two rules. 步骤 1（用户在设置中配置目标应用）is
//! [`crate::config`]'s `send_to_apps` and the parse/validate pipeline that reads it; 步骤 2
//! （完成截图后点击「发送到应用」）is a toolbar cell. What is left — and what had no owner
//! anywhere in this crate before this module — is 步骤 3「系统将图片作为剪贴板内容、文件参数
//! 或输入对象传递给目标应用」and 规则 2「目标程序不存在时应提示重新设置路径」.
//!
//! 步骤 3 is a *choice* before it is a piece of work: three arms are named and the program can
//! only take one of them per gesture. The work — writing the bytes, asking the shell — belongs
//! to the bridge and to `platform_windows::open`, which own the picture and the shell. What
//! belongs here is which arm, whether the target is on this machine at all, and how the path
//! is spelled into an argument the target's own command-line parser will read as one name.

use std::path::{Path, PathBuf, MAIN_SEPARATOR};

/// 步骤 3 点名的那三臂。
///
/// 三枚都在这里，是因为「选哪一臂」这一句在本仓库只许出现一次（第 281 项）；把它们写成枚举而
/// 不是直接落一臂，是为了让另外两臂的去向也能由这一处说明，而不是由三个调用点各猜各的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handover {
    /// 剪贴板内容。这一臂**早就有主**：§5.5.2 的 复制 与 §5.9.17 的「复制图片时同时提供文件」
    /// 走的是 `platform_windows::clip::publish`，交出去的是 CF_DIB + PNG（+ CF_HDROP）。
    /// 本模块不改判它，只是把它列在这里，好让「为什么这一次不走这一臂」有处可写。
    Clipboard,
    /// 文件参数：`exe "path"`。[`plan`] 唯一构造得出的那一臂。
    FileArgument,
    /// 输入对象：把图作为一次 drop 递进去。这一臂要**目标那一头**声明它接 drop，而本产品
    /// 一句那样的代码也没有（一次真的 drag-and-drop 也只能由人手做）—— 所以它在这一版里
    /// 只是被点名，不被使用。
    InputObject,
}

impl Handover {
    /// 需求里的原词，用来把选中的那一臂说回给用户和门听。三臂各有各的中文名，写在一处。
    pub fn label(&self) -> &'static str {
        match self {
            Handover::Clipboard => "剪贴板内容",
            Handover::FileArgument => "文件参数",
            Handover::InputObject => "输入对象",
        }
    }
}

/// 步骤 3 与规则 2 的答。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// 递得出去：目标在这台机器上定位到了，参数也按 shell 的规则拼好了。
    ///
    /// `exe` 是**定位到的那一个**（可能是配置里那句原话，也可能是 PATH 里找到的全路径），
    /// `path` 是交给目标的那枚文件，`parameters` 是 shell 那一头真正收到的串。把三者都带回去，
    /// 是因为门要判「程序收到的是不是我们指的那一枚」——只带回一个布尔，那一格就没法问。
    Dispatch {
        arm: Handover,
        exe: String,
        parameters: String,
        path: String,
    },
    /// 规则 2 的那一格：目标程序不在这台机器上，要用户**重新设置路径**。
    ///
    /// 话由本模块拼（`prompt`），因为那一句是需求自己要求的，而它出现在几扇门里只许有一句。
    Repath {
        name: String,
        exe: String,
        prompt: String,
    },
    /// 交不出去，而理由不是「目标没了」：没有可递的文件，或那路径拼不出一条安全参数。
    Refused { name: String, why: String },
}

/// 只写了文件名时按 `dirs` 找；写了路径就只看那一处。`pub` 是给门的：它要把自己那一枚
/// receiver 目录递进来问一句「这枚找得到吗」，而不必把下面这五行再抄一遍。
pub fn locate_in(exe: &str, dirs: &[PathBuf]) -> Option<String> {
    let want = exe.trim();
    if want.is_empty() {
        return None;
    }
    if want.contains('/') || want.contains('\\') || want.contains(MAIN_SEPARATOR) {
        // 路径写全了：盘上有就算，没有就是没有 —— 不替用户猜另一个目录，也不猜扩展名。
        return Path::new(want).is_file().then(|| want.to_string());
    }
    dirs.iter()
        .map(|dir| dir.join(want))
        .find(|hit| hit.is_file())
        .and_then(|hit| hit.to_str().map(str::to_string))
}

/// [`locate_in`] 挂上这台机器：搜索目录就是 `PATH`。找不到 ⇒ `None`，而 `None` 在 [`plan`]
/// 里直接长成规则 2 那一句提示。
pub fn locate(exe: &str) -> Option<String> {
    let dirs = match std::env::var_os("PATH") {
        Some(list) => std::env::split_paths(&list).collect::<Vec<_>>(),
        // 没有 PATH 这一枚变量：只写文件名的那一类条目一概定位不到，路径写全的那一类照判。
        None => Vec::new(),
    };
    locate_in(exe, &dirs)
}

/// 把一枚文件路径拼成 shell 那一头收到的参数。
///
/// 必须加引号：`ShellExecuteW` 的 `parameters` 是**原样接到目标自己的命令行**上的，而路径里
/// 可以有空格（`%TEMP%` 在不少机器上就有）。不能加引号的那一路是拒绝而不是替换：路径里已经
/// 带着一枚 `"` 时，任何补引号的写法都会让目标看见一对提前合拢的引号加一段我们没写过的词 ——
/// 那正是 §5.6.2 那只手绕开 `cmd /C start` 的同一个理由（外来数据不该再过一遍命令行解析）。
pub fn quote(path: &str) -> Result<String, String> {
    if path.trim().is_empty() {
        return Err("没有可递出去的图像文件：这一次点击手里是空的。".to_string());
    }
    if path.contains('"') {
        return Err(
            "那枚文件的路径里有引号，拼不出一条安全参数：请把截图存到不含引号的路径里。"
                .to_string(),
        );
    }
    Ok(format!("\"{path}\""))
}

/// §5.5.10 步骤 3 与规则 2，一次判完。
///
/// `name` 是配置里那条条目的显示名（也允许为空，此时话里用 `exe`），`exe` 是配置里那枚可执行
/// 文件，`path` 是本流程已经落到盘上的那枚图片。**两问的次序是有意为之**：先问有没有东西可递
/// （`quote`），再问对面在不在（`locate`，它要摸盘）——一枚递不出去的图片不该先花一次 PATH 查找，
/// 而「目标不存在」那一格是需求要用户**改设置**的，只有在前一问已经答过「有东西」之后才说得清。
pub fn plan(name: &str, exe: &str, path: &str) -> Delivery {
    let shown = {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            exe.to_string()
        } else {
            trimmed.to_string()
        }
    };
    let parameters = match quote(path) {
        Err(why) => return Delivery::Refused { name: shown, why },
        Ok(spelled) => spelled,
    };
    match locate(exe) {
        Some(found) => Delivery::Dispatch {
            // 三臂里取得出这一臂的理由写在 [`Handover`] 上：剪贴板那一臂归 §5.5.2/§5.9.17，
            // 输入对象那一臂要目标自己接 drop（本产品 0 处那样的代码），只有文件参数是
            // 「不必知道对面是谁」也成立的一臂。
            arm: Handover::FileArgument,
            exe: found,
            parameters,
            path: path.to_string(),
        },
        None => Delivery::Repath {
            prompt: format!("{shown} 的目标程序 {exe} 不在这台机器上：请在设置里重新设置路径。"),
            name: shown,
            exe: exe.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三臂都被点名，而取出去的只有文件参数那一枚：这一格钉的是「选哪一臂」在本仓库只写了
    /// 一次，且那一处选的确实是步骤 3 里的三臂之一。
    #[test]
    fn step_three_names_three_arms_and_takes_one() {
        let me = std::env::current_exe().unwrap();
        let path = me.to_str().unwrap().to_string();
        let got = plan(
            "记事本",
            "falconshot-no-such-target-on-this-machine.exe",
            &path,
        );
        assert!(matches!(got, Delivery::Repath { .. }), "wrong: {got:?}");

        match plan("记事本", path.as_str(), &path) {
            Delivery::Dispatch { arm, .. } => assert_eq!(arm, Handover::FileArgument),
            other => panic!("the local exe should dispatch, got {other:?}"),
        }
        assert_eq!(Handover::Clipboard.label(), "剪贴板内容");
        assert_eq!(Handover::FileArgument.label(), "文件参数");
        assert_eq!(Handover::InputObject.label(), "输入对象");
    }

    /// 规则 2 的那一句必须是需求自己的那一句：它要点名「重新设置路径」，也得点名**是哪一条**
    /// 条目坏了 —— 配置里可以有多枚目标，一句「目标不存在」不足以让人去改对的那一格。
    #[test]
    fn a_missing_target_asks_for_a_new_path() {
        let me = std::env::current_exe().unwrap();
        let got = plan(
            "画图",
            "falconshot-definitely-not-installed.exe",
            me.to_str().unwrap(),
        );
        match got {
            Delivery::Repath { name, exe, prompt } => {
                assert_eq!(name, "画图");
                assert_eq!(exe, "falconshot-definitely-not-installed.exe");
                assert!(prompt.contains("重新设置路径"), "the sentence: {prompt}");
                assert!(prompt.contains("画图"), "must name the entry: {prompt}");
            }
            other => panic!("wrong arm: {other:?}"),
        }
    }

    /// 有东西可递是比「对面在不在」更前面的一问：空路径不谈 spawn，带引号的路径也不谈 ——
    /// 后者尤其不能靠补一枚引号蒙过去，那是把一段我们没有写过的词交给对面的解析器。
    #[test]
    fn a_path_that_will_not_quote_is_refused_rather_than_spliced() {
        assert_eq!(
            quote(""),
            Err("没有可递出去的图像文件：这一次点击手里是空的。".to_string())
        );
        let err = quote("C:\\a\"b.png").unwrap_err();
        assert!(err.contains("引号"), "the sentence: {err}");
        assert_eq!(
            quote("C:\\My Shots\\a b.png").unwrap(),
            "\"C:\\My Shots\\a b.png\""
        );

        let got = plan("记事本", "notepad.exe", "C:\\a\"b.png");
        match got {
            Delivery::Refused { name, why } => {
                assert_eq!(name, "记事本");
                assert!(why.contains("引号"), "the sentence: {why}");
            }
            other => panic!("a quoting failure must not become a dispatch: {other:?}"),
        }
    }

    /// 搜索目录这一问是纯的：目录由调用方递进来，所以这一格测的是「只写文件名时要按目录找、
    /// 找到了交出的是**那一处**的全路径」，而不是这台机器的 PATH 此刻长什么样。
    #[test]
    fn a_bare_name_is_looked_up_in_the_dirs_given() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-sendto-fixture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("receiver.cmd"), "@echo off\r\n").unwrap();

        assert_eq!(
            locate_in("receiver.cmd", &[dir.clone()]).as_deref(),
            dir.join("receiver.cmd").to_str()
        );
        assert_eq!(locate_in("receiver.cmd", &[]), None);
        assert_eq!(locate_in("", &[dir.clone()]), None);
        // 路径写全了就不再翻目录：不存在的那一枚回 None，而不是"在 PATH 的某处"。
        assert_eq!(
            locate_in(&dir.join("nope.exe").to_string_lossy(), &[dir.clone()]),
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 写全路径的条目：在盘上就定位到原句，不在就是规则 2。这一格钉的是本模块**不**替用户
    /// 补扩展名、也不换目录 —— 那种好意会让提示里说出的路径与设置里那一句不是同一枚。
    #[test]
    fn a_full_path_is_taken_as_it_is_spelled() {
        let me = std::env::current_exe().unwrap();
        let spelled = me.to_str().unwrap();
        assert_eq!(locate_in(spelled, &[]).as_deref(), Some(spelled));

        let mut elsewhere = me.clone();
        elsewhere.set_file_name("falconshot-not-this-one.exe");
        assert_eq!(locate_in(&elsewhere.to_string_lossy(), &[]), None);
        match plan("", elsewhere.to_str().unwrap(), spelled) {
            // 名字空着 ⇒ 话里用 exe，而不是凭空造一个「未命名」。
            Delivery::Repath { name, prompt, .. } => {
                assert_eq!(name, elsewhere.to_str().unwrap());
                assert!(prompt.contains("重新设置路径"), "the sentence: {prompt}");
            }
            other => panic!("wrong arm: {other:?}"),
        }
    }

    /// `Dispatch` 带回三枚字符串，门要逐枚读，所以它们必须是**各自该是谁就是谁**：递出去的
    /// 是定位到的那枚 exe、带引号的那条参数、以及盘上那枚原文件 —— 少一枚就说不清谁收到了什么。
    #[test]
    fn a_dispatch_carries_target_argument_and_file() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-sendto-arm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("receiver.cmd"), "@echo off\r\n").unwrap();
        let picture = dir.join("shot.png");
        std::fs::write(&picture, b"not a png, but a real file").unwrap();

        let got = plan(
            "记事本",
            "falconshot-no-such-target.exe",
            picture.to_str().unwrap(),
        );
        assert!(matches!(got, Delivery::Repath { .. }));

        let exe = dir.join("receiver.cmd");
        match plan("记事本", exe.to_str().unwrap(), picture.to_str().unwrap()) {
            Delivery::Dispatch {
                exe: used,
                parameters,
                path,
                ..
            } => {
                assert_eq!(used, exe.to_str().unwrap());
                assert_eq!(path, picture.to_str().unwrap());
                assert_eq!(parameters, format!("\"{}\"", picture.to_str().unwrap()));
            }
            other => panic!("wrong arm: {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 配置里那句条目常被多打一个空格（`name| C:\a b\tool.exe` 是有人真会写出来的形状），
    /// 所以定位之前先 trim；而 trim 之后递给 shell 的也必须是 trim 过的那一枚，否则
    /// 「提示里说出的路径」与「真被启动的那一枚」不是同一句。
    #[test]
    fn spaces_around_the_entry_are_not_part_of_its_name() {
        let me = std::env::current_exe().unwrap();
        let spelled = me.to_str().unwrap();
        assert_eq!(locate(&format!("  {spelled}  ")).as_deref(), Some(spelled));

        let mut dir = std::env::temp_dir();
        dir.push(format!("falconshot-sendto-trim-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("receiver.cmd"), "@echo off\r\n").unwrap();
        assert_eq!(
            locate_in("  receiver.cmd ", &[dir.clone()]).as_deref(),
            dir.join("receiver.cmd").to_str()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
