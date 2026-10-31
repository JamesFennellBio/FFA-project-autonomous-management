// The whole app is Rust: start_app() (tauri::mobile_entry_point in
// app/src-tauri/src/lib.rs) runs UIApplicationMain and never returns.

@_silgen_name("start_app")
func startApp()

@main
enum FarmLab {
    static func main() {
        startApp()
    }
}
