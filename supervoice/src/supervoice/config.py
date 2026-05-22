from pydantic import SecretStr
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    """Runtime configuration loaded from environment variables."""

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    host: str = "0.0.0.0"
    port: int = 8080

    agent_bridge_url: str
    agent_bridge_reconnect_max_attempts: int = 5
    agent_bridge_reconnect_initial_delay_ms: int = 200
    webrtc_handshake_timeout_s: int = 10

    deepgram_api_key: SecretStr
    cartesia_api_key: SecretStr
    elevenlabs_api_key: SecretStr | None = None

    idle_warning_timeout_s: int = 30
    idle_disconnect_timeout_s: int = 60

    call_bearer_token: SecretStr | None = None
