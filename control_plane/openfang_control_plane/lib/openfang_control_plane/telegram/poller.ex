defmodule OpenfangControlPlane.Telegram.Poller do
  use GenServer
  require Logger

  alias OpenfangControlPlane.Telegram.Client
  alias OpenfangControlPlane.OpenFang.HTTP
  alias OpenfangControlPlane.Jobs

  @poll_timeout 30
  @poll_receive_timeout_ms (@poll_timeout + 10) * 1_000

  def start_link(_args) do
    GenServer.start_link(__MODULE__, %{}, name: __MODULE__)
  end

  @impl true
  def init(_state) do
    if Client.token() == "" do
      Logger.error("TELEGRAM_BOT_TOKEN not set; control plane will not run")
      :ignore
    else
      allowed = allowed_users()
      if allowed == [] do
        Logger.error("TELEGRAM_ALLOWED_USERS not set; entering bootstrap mode (only /id works)")
        state = %{offset: 0, allowed: :bootstrap}
        send(self(), :poll)
        {:ok, state}
      else
        state = %{offset: 0, allowed: allowed}
        send(self(), :poll)
        {:ok, state}
      end
    end
  end

  @impl true
  def handle_info(:poll, state) do
    started = System.monotonic_time(:millisecond)

    res =
      Req.get!(Client.api_base() <> "/getUpdates",
        # Telegram long-polling can legitimately take `timeout` seconds; ensure
        # the HTTP client doesn't time out first.
        receive_timeout: @poll_receive_timeout_ms,
        params: %{
          "timeout" => @poll_timeout,
          "offset" => state.offset,
          "allowed_updates" => Jason.encode!(["message"])
        }
      ).body

    updates = Map.get(res, "result", [])
    new_offset =
      updates
      |> Enum.map(&Map.get(&1, "update_id", 0))
      |> Enum.max(fn -> state.offset end)
      |> Kernel.+(1)

    Enum.each(updates, fn upd -> handle_update(upd, state.allowed) end)

    duration_ms = System.monotonic_time(:millisecond) - started
    Logger.info(fn ->
      Jason.encode!(%{event: "tg.poll", outcome: "ok", duration_ms: duration_ms, updates: length(updates)})
    end)

    Process.send_after(self(), :poll, 0)
    {:noreply, %{state | offset: max(state.offset, new_offset)}}
  rescue
    e ->
      Logger.error(fn ->
        Jason.encode!(%{event: "tg.poll", outcome: "error", error: Exception.message(e)})
      end)
      Process.send_after(self(), :poll, 2_000)
      {:noreply, state}
  end

  defp handle_update(%{"message" => msg}, allowed) do
    chat_id = msg |> get_in(["chat", "id"]) |> to_string()
    user_id = msg |> get_in(["from", "id"]) |> to_string()

    text = Map.get(msg, "text", "") |> to_string()

    cond do
      allowed == :bootstrap ->
        handle_bootstrap(chat_id, user_id, text)

      allowed != [] and not (user_id in allowed) ->
        :ok

      true ->
        handle_command(chat_id, user_id, text)
    end
  end

  defp handle_update(_other, _allowed), do: :ok

  defp handle_bootstrap(chat_id, user_id, text) do
    {cmd, _args} = parse_command(text)

    case cmd do
      "/id" ->
        Client.send_message(
          chat_id,
          "Your Telegram user id is: `#{user_id}`\n\nSet `TELEGRAM_ALLOWED_USERS=#{user_id}` in `~/.openfang/control_plane.env` on the remote host, then restart the service."
        )

      "/help" ->
        Client.send_message(
          chat_id,
          "Bootstrap mode: only `/id` works until you set `TELEGRAM_ALLOWED_USERS`."
        )

      _ ->
        :ok
    end
  end

  defp handle_command(chat_id, user_id, text) do
    started = System.monotonic_time(:millisecond)
    {cmd, args} = parse_command(text)

    # Track an "admin" chat so scheduled jobs know where to report.
    OpenfangControlPlane.Telegram.AdminState.record(chat_id, user_id)

    result =
      case cmd do
        "/id" ->
          # Always allow `/id` so users can recover their numeric ID even after
          # allowlisting is enabled.
          Client.send_message(chat_id, "Your Telegram user id is: `#{user_id}`")
          :ok

        "/help" ->
          Client.send_message(chat_id, help_text())
          :ok

        "/agents" ->
          agents = HTTP.list_agents()
          Client.send_message(chat_id, render_agents(agents))
          :ok

        "/run" ->
          case parse_run_args(args) do
            {:ok, agent_ref, task} ->
              Task.Supervisor.start_child(OpenfangControlPlane.TaskSupervisor, fn ->
                OpenfangControlPlane.Runner.run(chat_id, user_id, agent_ref, task)
              end)

              :ok

            {:error, msg} ->
              Client.send_message(chat_id, msg)
              :error
          end

        "/status" ->
          id = String.trim(args || "")
          case Jobs.get(id) do
            nil -> Client.send_message(chat_id, "*Unknown job id*"); :error
            job -> Client.send_message(chat_id, render_job(job)); :ok
          end

        "/stop" ->
          id = String.trim(args || "")
          Task.Supervisor.start_child(OpenfangControlPlane.TaskSupervisor, fn ->
            OpenfangControlPlane.Runner.stop_job(chat_id, id)
          end)
          :ok

        "/logs" ->
          list = Jobs.latest_for_chat(chat_id, 10)
          Client.send_message(chat_id, render_recent(list))
          :ok

        _ ->
          # ignore unknown to reduce spam
          :ignore
      end

    duration_ms = System.monotonic_time(:millisecond) - started
    Logger.info(fn ->
      Jason.encode!(%{
        event: "tg.command",
        cmd: cmd,
        chat_id: chat_id,
        user_id: user_id,
        outcome: to_string(result),
        duration_ms: duration_ms
      })
    end)

    :ok
  rescue
    e ->
      Logger.error(fn ->
        Jason.encode!(%{event: "tg.command", outcome: "error", error: Exception.message(e)})
      end)

      # Avoid silent failures (e.g. OpenFang unreachable from Docker). Keep the
      # message generic to avoid leaking any sensitive details.
      Client.send_message(
        chat_id,
        "Command failed on the server. Try again in a few seconds. If it keeps failing, check logs on the GPU box: `journalctl --user -u openfang_control_plane -n 80 --no-pager`"
      )

      :ok
  end

  defp allowed_users do
    System.get_env("TELEGRAM_ALLOWED_USERS", "")
    |> String.split(",", trim: true)
    |> Enum.map(&String.trim/1)
    |> Enum.reject(&(&1 == ""))
  end

  defp parse_command(text) do
    t = (text || "") |> String.trim()
    case String.split(t, " ", parts: 2) do
      [cmd] -> {cmd, ""}
      [cmd, rest] -> {cmd, rest}
      _ -> {"", ""}
    end
  end

  defp parse_run_args(args) do
    parts = String.split(args || "", " ", parts: 2, trim: true)
    case parts do
      [agent_ref, task] when task != "" -> {:ok, agent_ref, task}
      _ -> {:error, "Usage: `/run <agent_name_or_id> <task...>`"}
    end
  end

  defp help_text do
    """
Available commands:
`/agents` - list agents
`/run <agent> <task>` - run a task
`/status <job_id>` - show job status
`/stop <job_id>` - stop a job
`/logs` - recent jobs
`/help` - this help
"""
  end

  defp render_agents(agents) when is_list(agents) do
    header = "*Agents*"
    body =
      agents
      |> Enum.take(30)
      |> Enum.map(fn a ->
        name = a["name"] || "agent"
        id = a["id"] || "?"
        prov = a["model_provider"] || "?"
        model = a["model_name"] || "?"
        "- `#{name}` (`#{id}`) `#{prov}:#{model}`"
      end)
      |> Enum.join("\n")
    header <> "\n" <> body
  end

  defp render_agents(_), do: "No agents."

  defp render_job(job) do
    """
*Job* `#{job.id}`
status: `#{job.status}`
agent_id: `#{job.agent_id}`
provider/model: `#{job.requested_provider || "?"}:#{job.requested_model || "?"}`
started_at: `#{fmt_dt(job.started_at)}`
finished_at: `#{fmt_dt(job.finished_at)}`
summary: #{job.result_summary || ""}
error: #{job.error || ""}
"""
  end

  defp render_recent(list) do
    header = "*Recent jobs*"
    body =
      list
      |> Enum.map(fn j ->
        "- `#{j.id}` `#{j.status}` agent=`#{j.agent_id}`"
      end)
      |> Enum.join("\n")
    header <> "\n" <> body
  end

  defp fmt_dt(nil), do: ""
  defp fmt_dt(dt), do: DateTime.to_iso8601(dt)
end
