"""
API Key Encryption Utility
Provides encryption/decryption for sensitive API keys
"""
from cryptography.fernet import Fernet
from pathlib import Path
from typing import Optional
import os

from app.utils.logging import get_logger

logger = get_logger(__name__)


class APIKeyEncryption:
    """
    API key encryption using Fernet symmetric encryption.

    Key is stored in data/.encryption_key file.
    """

    def __init__(self, key_file: Optional[str] = None):
        """
        Initialize encryption utility.

        Args:
            key_file: Path to encryption key file (default: data/.encryption_key)
        """
        self.key_file = Path(key_file or "data/.encryption_key")
        self._key = self._get_or_create_key()
        self._fernet = Fernet(self._key)

    def _get_or_create_key(self) -> bytes:
        """
        Get existing encryption key or create a new one.

        Returns:
            Encryption key as bytes
        """
        if self.key_file.exists():
            try:
                with open(self.key_file, "rb") as f:
                    key = f.read()
                # Validate key
                Fernet(key)
                return key
            except Exception as e:
                logger.warning(
                    "Failed to load encryption key, creating new one",
                    error=str(e)
                )

        # Create new key
        key = Fernet.generate_key()

        # Ensure directory exists
        self.key_file.parent.mkdir(parents=True, exist_ok=True)

        # Save key with restricted permissions
        with open(self.key_file, "wb") as f:
            f.write(key)

        # Set file permissions (owner read/write only)
        try:
            os.chmod(self.key_file, 0o600)
        except Exception:
            pass  # Windows doesn't support chmod

        logger.info("Created new encryption key", key_file=str(self.key_file))
        return key

    def encrypt(self, api_key: str) -> str:
        """
        Encrypt an API key.

        Args:
            api_key: Plain text API key

        Returns:
            Encrypted API key as base64 string
        """
        if not api_key:
            return ""

        encrypted = self._fernet.encrypt(api_key.encode("utf-8"))
        return encrypted.decode("utf-8")

    def decrypt(self, encrypted: str) -> str:
        """
        Decrypt an encrypted API key.

        Args:
            encrypted: Encrypted API key as base64 string

        Returns:
            Decrypted plain text API key
        """
        if not encrypted:
            return ""

        try:
            decrypted = self._fernet.decrypt(encrypted.encode("utf-8"))
            return decrypted.decode("utf-8")
        except Exception as e:
            logger.error("Failed to decrypt API key", error=str(e))
            raise ValueError("Failed to decrypt API key")

    def rotate_key(self) -> None:
        """
        Rotate encryption key (re-encrypts all data).

        Note: Caller is responsible for re-encrypting stored keys.
        """
        old_key = self._key
        self._key = Fernet.generate_key()
        self._fernet = Fernet(self._key)

        # Save new key
        with open(self.key_file, "wb") as f:
            f.write(self._key)

        logger.info("Rotated encryption key")
        return old_key


# Global instance
_encryption: Optional[APIKeyEncryption] = None


def get_encryption() -> APIKeyEncryption:
    """Get or create global encryption instance."""
    global _encryption
    if _encryption is None:
        _encryption = APIKeyEncryption()
    return _encryption
