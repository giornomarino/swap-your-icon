// Windows 发布版不弹控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 被自身以管理员身份调起：只执行任务文件里的操作，然后退出
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--elevated") {
        let code = args
            .get(i + 1)
            .map(|job| iconswap_lib::platform::run_elevated_job(job))
            .unwrap_or(2);
        std::process::exit(code);
    }
    iconswap_lib::run()
}
