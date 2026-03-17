defmodule OpenfangControlPlane.Telegram.AdminState do
  @moduledoc false

  use GenServer

  # Keeps the last seen allowlisted chat/user so scheduled jobs can report
  # somewhere without requiring you to hardcode a chat id in env vars.

  def start_link(_args) do
    GenServer.start_link(__MODULE__, %{chat_id: nil, user_id: nil}, name: __MODULE__)
  end

  @impl true
  def init(state), do: {:ok, state}

  def record(chat_id, user_id) when is_binary(chat_id) and is_binary(user_id) do
    GenServer.cast(__MODULE__, {:record, chat_id, user_id})
  end

  def get do
    GenServer.call(__MODULE__, :get)
  end

  @impl true
  def handle_cast({:record, chat_id, user_id}, _state) do
    {:noreply, %{chat_id: chat_id, user_id: user_id}}
  end

  @impl true
  def handle_call(:get, _from, state) do
    {:reply, state, state}
  end
end

