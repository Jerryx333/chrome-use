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
                ContentUnavailableView("No schedules", systemImage: "calendar", description: Text("Schedules are created in the side panel on the computer."))
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
                        let last = schedule.lastResult ?? String(localized: "not run yet")
                        Text("Last: \(last)")
                            .foregroundStyle(schedule.lastResult?.hasPrefix("failed") == true ? .red : .secondary)
                        if schedule.enabled, let next = schedule.nextRun {
                            let when = next.formatted(date: .abbreviated, time: .shortened)
                            Text("· Next: \(when)").foregroundStyle(.secondary)
                        }
                    }
                    .font(.caption)
                    HStack {
                        Button("Run now") { Task { await run(schedule) } }
                            .buttonStyle(.bordered)
                            .disabled(busy == schedule.id || schedule.running)
                        if let chat = schedule.lastChat {
                            Button("Open last run") { model.path.append(.conversation(client.id, chat)) }
                                .buttonStyle(.borderless)
                        }
                    }
                    .font(.callout)
                }
                .padding(.vertical, 4)
            }
            if let error { Text(error).font(.footnote).foregroundStyle(.red) }
        }
        .navigationTitle("Schedules")
        .refreshable { await load() }
        .task { await load() }
    }

    private func ruleText(_ rule: JSONValue) -> String {
        let minutes = rule["minutes"].int ?? 0
        let time = rule["time"].string ?? ""
        switch rule["kind"].string {
        case "interval": return String(localized: "Every \(minutes) minutes")
        case "daily": return String(localized: "Daily at \(time)")
        case "weekdays": return String(localized: "Weekdays at \(time)")
        case "cron": return rule["expression"].string ?? ""
        default: return ""
        }
    }

    private func load() async {
        do {
            schedules = try await client.schedules()
            error = nil
        } catch {
            self.error = describe(error, deviceId: client.id)
        }
        loaded = true
    }

    private func toggle(_ schedule: Schedule, _ on: Bool) async {
        guard model.allow(.toggleSchedule) else { return }
        busy = schedule.id
        defer { busy = nil }
        do { try await client.setSchedule(schedule.id, enabled: on) } catch { self.error = describe(error, deviceId: client.id) }
        await load()
    }

    private func run(_ schedule: Schedule) async {
        if let refusal = model.elevatedRefusal(client, elevated: schedule.elevated) { error = refusal; return }
        guard model.allow(.runSchedule) else { return }
        busy = schedule.id
        defer { busy = nil }
        do { try await client.runSchedule(schedule.id) } catch { self.error = describe(error, deviceId: client.id) }
        await load()
    }
}
