import ChromeUseKit
import SwiftUI

struct SchedulesView: View {
    @Environment(AppModel.self) private var model
    let client: GatewayClient
    @State private var schedules: [Schedule] = []
    @State private var loaded = false
    @State private var busy: String?
    @State private var error: String?

    var body: some View {
        List {
            if loaded && schedules.isEmpty {
                ContentUnavailableView("没有定时任务", systemImage: "calendar", description: Text("定时任务在电脑的侧边栏里创建。"))
                    .listRowBackground(Color.clear)
            }
            ForEach(schedules) { schedule in
                VStack(alignment: .leading, spacing: 6) {
                    Toggle(isOn: Binding(get: { schedule.enabled }, set: { on in Task { await toggle(schedule, on) } })) {
                        Text(schedule.name).font(.headline)
                    }
                    .disabled(busy == schedule.id)
                    Text([ruleText(schedule.rule), schedule.agentId].filter { !$0.isEmpty }.joined(separator: " · "))
                        .font(.caption).foregroundStyle(.secondary)
                    HStack {
                        let last = schedule.lastResult ?? String(localized: "还没运行")
                        Text("上次：\(last)")
                            .foregroundStyle(schedule.lastResult?.hasPrefix("failed") == true ? .red : .secondary)
                        if schedule.enabled, let next = schedule.nextRun {
                            let when = next.formatted(date: .abbreviated, time: .shortened)
                            Text("· 下次：\(when)").foregroundStyle(.secondary)
                        }
                    }
                    .font(.caption)
                    HStack {
                        Button("立即运行") { Task { await run(schedule) } }
                            .buttonStyle(.bordered)
                            .disabled(busy == schedule.id || schedule.running)
                        if let chat = schedule.lastChat {
                            Button("查看上次运行") { model.path.append(.conversation(client.id, chat)) }
                                .buttonStyle(.borderless)
                        }
                    }
                    .font(.callout)
                }
                .padding(.vertical, 4)
            }
            if let error { Text(error).font(.footnote).foregroundStyle(.red) }
        }
        .navigationTitle("定时任务")
        .refreshable { await load() }
        .task { await load() }
    }

    private func ruleText(_ rule: JSONValue) -> String {
        switch rule["kind"].string {
        case "interval": return String(localized: "每 \(rule["minutes"].int ?? 0) 分钟")
        case "daily": return String(localized: "每天 \(rule["time"].string ?? "")")
        case "weekdays": return String(localized: "工作日 \(rule["time"].string ?? "")")
        case "cron": return rule["expression"].string ?? ""
        default: return ""
        }
    }

    private func load() async {
        do {
            schedules = try await client.schedules()
            error = nil
        } catch {
            self.error = describe(error)
        }
        loaded = true
    }

    private func toggle(_ schedule: Schedule, _ on: Bool) async {
        guard model.requireWrite() else { return }
        busy = schedule.id
        defer { busy = nil }
        do { try await client.setSchedule(schedule.id, enabled: on) } catch { self.error = describe(error) }
        await load()
    }

    private func run(_ schedule: Schedule) async {
        guard model.requireWrite() else { return }
        busy = schedule.id
        defer { busy = nil }
        do { try await client.runSchedule(schedule.id) } catch { self.error = describe(error) }
        await load()
    }
}
