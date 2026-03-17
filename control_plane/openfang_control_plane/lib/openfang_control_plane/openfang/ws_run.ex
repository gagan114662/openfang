defmodule OpenfangControlPlane.OpenFang.WSRun do
  @moduledoc false

  require Logger

  # A short-lived WS client per run (simplest + robust).
  # Uses WebSockex and the OpenFang WS v1 JSON envelope.

  def run(agent_id, message, handlers) do
    ws_base = System.get_env("OPENFANG_WS_BASE", "ws://127.0.0.1:50051")
    token = System.get_env("OPENFANG_API_KEY", "")
    url =
      if token != "" do
        ws_base <> "/ws?token=" <> URI.encode_www_form(token)
      else
        ws_base <> "/ws"
      end

    state = %{
      agent_id: agent_id,
      message: message,
      handlers: handlers,
      stream_key: nil,
      last_seq: 0,
      frames_since_ack: 0,
      started: false
    }

    WebSockex.start_link(url, __MODULE__, state,
      extra_headers: [{"sec-websocket-protocol", "of-ws.v1.json"}]
    )
  end

  def handle_connect(_conn, state) do
    {:ok, state}
  end

  def handle_disconnect(map, state) do
    # Auto-reconnect so long runs survive transient disconnects.
    #
    # NOTE: WebSockex supports returning {:reconnect, state} to retry the
    # connection. On reconnect we will send control.resume in dispatch/2 once
    # we receive control.hello.
    if state.handlers[:on_error] do
      state.handlers[:on_error].(%{"code" => "ws_disconnected", "details" => map})
    end

    backoff_ms =
      case Map.get(state, :reconnect_attempts, 0) do
        n when n < 1 -> 250
        n when n < 2 -> 500
        n when n < 3 -> 1_000
        n when n < 4 -> 2_000
        _ -> 5_000
      end

    attempts = Map.get(state, :reconnect_attempts, 0) + 1
    Process.sleep(backoff_ms)
    {:reconnect, Map.put(state, :reconnect_attempts, attempts)}
  end

  def handle_frame({:text, msg}, state) do
    with {:ok, frame} <- Jason.decode(msg) do
      state = maybe_track_seq(state, frame)
      dispatch(frame, state)
    else
      _ -> {:ok, state}
    end
  end

  def handle_frame(_frame, state), do: {:ok, state}

  defp dispatch(%{"topic" => "control", "op" => "hello"}, state) do
    # On first connect, start the run. On reconnect, resume the stream if possible.
    cond do
      state.started and is_binary(state.stream_key) ->
        resume = %{"streams" => [%{"key" => state.stream_key, "last_seq" => state.last_seq}]}

        frame = %{
          "v" => 1,
          "id" => UUID.uuid4(),
          "ts" => nil,
          "topic" => "control",
          "op" => "resume",
          "seq" => nil,
          "key" => nil,
          "payload" => resume
        }

        {:reply, {:text, Jason.encode!(frame)}, state}

      true ->
        id = UUID.uuid4()
        payload = %{"agent_id" => state.agent_id, "message" => state.message, "stream" => true}
        frame = %{
          "v" => 1,
          "id" => id,
          "ts" => nil,
          "topic" => "agent.run",
          "op" => "start",
          "seq" => nil,
          "key" => nil,
          "payload" => payload
        }
        {:reply, {:text, Jason.encode!(frame)}, %{state | started: true}}
    end
  end

  defp dispatch(%{"topic" => "control", "op" => "resumed"}, state) do
    # After resume, re-grant credits so delivery can continue.
    if is_binary(state.stream_key) do
      credits = %{"topic" => "agent.run", "key" => state.stream_key, "grant" => 128}
      frame = %{"v" => 1, "id" => UUID.uuid4(), "ts" => nil, "topic" => "control", "op" => "credits", "seq" => nil, "key" => nil, "payload" => credits}
      {:reply, {:text, Jason.encode!(frame)}, state}
    else
      {:ok, state}
    end
  end

  defp dispatch(%{"topic" => "agent.run", "op" => "start", "key" => key}, state) when is_binary(key) do
    # grant credits for this stream
    credits = %{"topic" => "agent.run", "key" => key, "grant" => 128}
    frame = %{"v" => 1, "id" => UUID.uuid4(), "ts" => nil, "topic" => "control", "op" => "credits", "seq" => nil, "key" => nil, "payload" => credits}
    handlers = state.handlers
    if handlers[:on_start], do: handlers[:on_start].(key)
    {:reply, {:text, Jason.encode!(frame)}, %{state | stream_key: key}}
  end

  defp dispatch(%{"topic" => "agent.run", "op" => "delta", "payload" => %{"text" => text}}, state) do
    if state.handlers[:on_delta], do: state.handlers[:on_delta].(text)
    {:ok, state}
  end

  defp dispatch(%{"topic" => "agent.run", "op" => "tool", "payload" => payload}, state) do
    if state.handlers[:on_tool], do: state.handlers[:on_tool].(payload)
    {:ok, state}
  end

  defp dispatch(%{"topic" => "agent.run", "op" => "result", "payload" => payload}, state) do
    if state.handlers[:on_result], do: state.handlers[:on_result].(payload)
    {:ok, state}
  end

  defp dispatch(%{"topic" => "agent.run", "op" => "done", "payload" => payload}, state) do
    if state.handlers[:on_done], do: state.handlers[:on_done].(payload, state.last_seq, state.stream_key)
    {:close, state}
  end

  defp dispatch(%{"topic" => "control", "op" => "error", "payload" => payload}, state) do
    if state.handlers[:on_error], do: state.handlers[:on_error].(payload)
    {:ok, state}
  end

  defp dispatch(_frame, state), do: {:ok, state}

  defp maybe_track_seq(state, %{"key" => key, "seq" => seq}) when is_binary(key) and is_integer(seq) do
    state = %{state | last_seq: max(state.last_seq, seq), frames_since_ack: state.frames_since_ack + 1}

    if state.stream_key == key and state.frames_since_ack >= 32 do
      ack = %{"key" => key, "last_seq" => state.last_seq, "grant" => 64}
      frame = %{"v" => 1, "id" => UUID.uuid4(), "ts" => nil, "topic" => "control", "op" => "ack", "seq" => nil, "key" => nil, "payload" => ack}
      # Send ack+credits without interfering with any reply the dispatcher may return.
      _ = WebSockex.send_frame(self(), {:text, Jason.encode!(frame)})
      %{state | frames_since_ack: 0}
    else
      state
    end
  end

  defp maybe_track_seq(state, _), do: state
end
