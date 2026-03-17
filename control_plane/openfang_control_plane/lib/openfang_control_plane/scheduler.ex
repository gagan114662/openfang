defmodule OpenfangControlPlane.Scheduler do
  @moduledoc false

  use GenServer
  require Logger

  alias OpenfangControlPlane.Telegram.AdminState
  alias OpenfangControlPlane.Runner
  alias OpenfangControlPlane.Jobs

  # This is deliberately simple: no external scheduling deps, and it respects:
  # - a max-running cap
  # - "once per day" semantics
  # - local OS timezone (remote host should be set to your timezone)

  def start_link(_args) do
    GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  end

  @impl true
  def init(_state) do
    enabled =
      System.get_env("OFCP_NIGHTLY_ENABLED", "false")
      |> String.downcase()
      |> then(&(&1 in ["1", "true", "yes", "y"]))

    state = %{
      enabled: enabled,
      last_run_date: nil
    }

    send(self(), :tick)
    {:ok, state}
  end

  @impl true
  def handle_info(:tick, state) do
    state =
      if state.enabled do
        maybe_run_nightly(state)
      else
        state
      end

    Process.send_after(self(), :tick, 60_000)
    {:noreply, state}
  end

  defp maybe_run_nightly(state) do
    # localtime: {{yyyy,mm,dd},{hh,mm,ss}}
    {{y, m, d}, {hh, mm, _ss}} = :calendar.local_time()
    today = {y, m, d}

    run_hh = env_int("OFCP_NIGHTLY_HH", 1)
    run_mm = env_int("OFCP_NIGHTLY_MM", 0)
    max_running = env_int("OFCP_MAX_RUNNING_JOBS", 2)

    cond do
      state.last_run_date == today ->
        state

      hh != run_hh or mm != run_mm ->
        state

      Jobs.count_running() >= max_running ->
        Logger.info(fn ->
          Jason.encode!(%{
            event: "nightly.skip",
            reason: "max_running",
            max_running: max_running,
            running: Jobs.count_running()
          })
        end)

        state

      true ->
        agent_ref = System.get_env("OFCP_NIGHTLY_AGENT", "debugger")

        task =
          System.get_env(
            "OFCP_NIGHTLY_TASK",
            "Run a health sweep of the OpenFang repo. Use tools to: (1) `cd ~/open_fang` (2) `cargo test --workspace` (3) `cargo clippy --workspace --all-targets`. Then summarize failures (if any) in a short bullet list."
          )

        admin = AdminState.get()

        if admin.chat_id && admin.user_id do
          Logger.info(fn ->
            Jason.encode!(%{
              event: "nightly.run",
              agent: agent_ref,
              chat_id: admin.chat_id,
              max_running: max_running
            })
          end)

          Task.Supervisor.start_child(OpenfangControlPlane.TaskSupervisor, fn ->
            Runner.run(admin.chat_id, admin.user_id, agent_ref, task)
          end)

          %{state | last_run_date: today}
        else
          Logger.info(fn ->
            Jason.encode!(%{event: "nightly.skip", reason: "no_admin_chat"})
          end)

          state
        end
    end
  end

  defp env_int(name, default) do
    case System.get_env(name, "") |> String.trim() do
      "" -> default
      s ->
        case Integer.parse(s) do
          {n, _} -> n
          :error -> default
        end
    end
  end
end

