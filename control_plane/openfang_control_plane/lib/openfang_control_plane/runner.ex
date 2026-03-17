defmodule OpenfangControlPlane.Runner do
  require Logger

  alias OpenfangControlPlane.Telegram.Client
  alias OpenfangControlPlane.OpenFang.HTTP
  alias OpenfangControlPlane.Jobs
  alias OpenfangControlPlane.Jobs.Job
  alias OpenfangControlPlane.OpenFang.WSRun

  def run(chat_id, user_id, agent_ref, task) do
    agents = HTTP.list_agents()
    agent = find_agent(agents, agent_ref)

    if is_nil(agent) do
      Client.send_message(chat_id, "*Unknown agent*: `#{agent_ref}`")
      :error
    else
      agent_id = agent["id"]
      provider = agent["model_provider"]
      model = agent["model_name"]

      start_msg =
        Client.send_message(
          chat_id,
          "*Run started* agent=`#{agent["name"]}`\n`#{provider}:#{model}`\n\nTask:\n#{task}"
        )
      message_id = get_in(start_msg, ["result", "message_id"]) || get_in(start_msg, ["message_id"])

      {:ok, job} =
        Jobs.create(%{
          telegram_chat_id: chat_id,
          telegram_user_id: user_id,
          telegram_message_id: message_id,
          agent_id: agent_id,
          status: "running",
          requested_provider: provider,
          requested_model: model,
          started_at: DateTime.utc_now()
        })

      # Update the starter message to include the job id for copy/paste.
      _ =
        Client.edit_message(
          chat_id,
          message_id,
          "*Run started* agent=`#{agent["name"]}` job=`#{job.id}`\n`#{provider}:#{model}`\n\nTask:\n#{task}"
        )

      buf = :ets.new(:buf, [:set, :private])
      :ets.insert(buf, {:text, ""})
      :ets.insert(buf, {:last_edit_ms, 0})
      :ets.insert(buf, {:tool_lines, []})

      handlers = %{
        on_start: fn key ->
          _ = Jobs.update(job, %{ws_stream_key: key})
        end,
        on_delta: fn text ->
          append(buf, :text, text)
          maybe_edit(chat_id, message_id, agent["name"], job.id, buf)
        end,
        on_tool: fn payload ->
          add_tool_line(buf, payload)
          maybe_edit(chat_id, message_id, agent["name"], job.id, buf)
        end,
        on_result: fn payload ->
          add_result_line(buf, payload)
          maybe_edit(chat_id, message_id, agent["name"], job.id, buf)
        end,
        on_error: fn payload ->
          Logger.error(fn -> Jason.encode!(%{event: "ws.error", job_id: job.id, payload: payload}) end)
        end,
        on_done: fn payload, last_seq, stream_key ->
          _ = Jobs.update(job, %{ws_last_seq: last_seq, ws_stream_key: stream_key})
          finish(chat_id, message_id, agent["name"], job, payload, buf)
        end
      }

      {:ok, _pid} = WSRun.run(agent_id, task, handlers)
      :ok
    end
  end

  def stop_job(chat_id, job_id) do
    case Jobs.get(job_id) do
      nil ->
        Client.send_message(chat_id, "*Unknown job id*")
        :error

      %Job{} = job ->
        _ = Jobs.update(job, %{status: "stopping"})
        _ = HTTP.stop_agent(job.agent_id)
        Client.send_message(chat_id, "*Stop sent* job=`#{job.id}` agent=`#{job.agent_id}`")
        :ok
    end
  end

  defp find_agent(agents, ref) when is_list(agents) do
    Enum.find(agents, fn a ->
      a["id"] == ref or String.downcase(to_string(a["name"] || "")) == String.downcase(ref)
    end)
  end

  defp append(buf, key, text) do
    [{^key, cur}] = :ets.lookup(buf, key)
    next = (cur <> text) |> tail_telegram()
    :ets.insert(buf, {key, next})
  end

  defp add_tool_line(buf, payload) do
    name = payload["name"] || "tool"
    id = payload["id"] || ""
    input = payload["input"]
    line =
      if is_nil(input) do
        "Tool start: `#{name}`"
      else
        "Tool end: `#{name}` id=`#{id}`"
      end
    push_line(buf, line)
  end

  defp add_result_line(buf, payload) do
    name = payload["name"] || "tool"
    is_err = payload["is_error"] || false
    content = payload["content"] || ""
    preview = content |> to_string() |> String.slice(0, 240)
    line = if is_err, do: "Tool result (error): `#{name}` #{preview}", else: "Tool result: `#{name}` #{preview}"
    push_line(buf, line)
  end

  defp push_line(buf, line) do
    [{:tool_lines, lines}] = :ets.lookup(buf, :tool_lines)
    next = ([line | lines] |> Enum.take(12)) |> Enum.reverse()
    :ets.insert(buf, {:tool_lines, next})
  end

  defp maybe_edit(chat_id, message_id, agent_name, job_id, buf) do
    now = System.monotonic_time(:millisecond)
    [{:last_edit_ms, last}] = :ets.lookup(buf, :last_edit_ms)
    if now - last < 1_000 do
      :ok
    else
      :ets.insert(buf, {:last_edit_ms, now})
      text = render_live(agent_name, job_id, buf)
      _ = Client.edit_message(chat_id, message_id, text)
      :ok
    end
  end

  defp finish(chat_id, message_id, agent_name, job, payload, buf) do
    finished = DateTime.utc_now()

    {status, summary, err} =
      if payload["error"] do
        {"error", nil, to_string(payload["error"])}
      else
        resp = payload["response"] || ""
        {"done", to_string(resp) |> String.slice(0, 800), nil}
      end

    _ = Jobs.update(job, %{status: status, finished_at: finished, result_summary: summary, error: err})

    final_text =
      if status == "done" do
        resp = payload["response"] || ""
        text = "*Done* agent=`#{agent_name}` job=`#{job.id}`\n\n" <> to_string(resp)
        text |> tail_telegram()
      else
        "*Error* agent=`#{agent_name}` job=`#{job.id}`\n\n" <> (err || "")
      end

    _ = Client.edit_message(chat_id, message_id, final_text)

    Logger.info(fn ->
      Jason.encode!(%{
        event: "job.complete",
        job_id: job.id,
        agent_id: job.agent_id,
        provider: job.requested_provider,
        model: job.requested_model,
        outcome: status,
        duration_ms: duration_ms(job.started_at, finished),
        error: err || ""
      })
    end)
  end

  defp render_live(agent_name, job_id, buf) do
    [{:text, text}] = :ets.lookup(buf, :text)
    [{:tool_lines, lines}] = :ets.lookup(buf, :tool_lines)
    tools =
      case lines do
        [] -> ""
        _ -> "\n\n*Tools*\n" <> Enum.join(lines, "\n")
      end
    ("*Running* agent=`#{agent_name}` job=`#{job_id}`\n\n" <> text <> tools)
    |> tail_telegram()
  end

  defp tail_telegram(s) do
    # Telegram limit is 4096 chars. Keep head info + tail of content.
    max = 3800
    str = to_string(s)
    if String.length(str) <= max do
      str
    else
      "...\n" <> String.slice(str, String.length(str) - max, max)
    end
  end

  defp duration_ms(nil, _), do: 0
  defp duration_ms(started_at, finished_at) do
    DateTime.diff(finished_at, started_at, :millisecond)
  end
end
