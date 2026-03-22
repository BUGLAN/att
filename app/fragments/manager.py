"""
Fragment management module
Handles storage, merging, and export of transcription fragments
"""
import aiosqlite
from typing import Optional, List
from datetime import datetime
from pathlib import Path
import json

from app.config import settings
from app.fragments.models import Fragment, FragmentStatus, Session, ExportFormat
from app.utils.logging import get_logger

logger = get_logger(__name__)


class FragmentManager:
    """
    Manages transcription fragments in SQLite database

    Features:
    - CRUD operations for fragments and sessions
    - Fragment merging with deduplication
    - Multiple export formats (TXT, JSON, SRT)
    """

    def __init__(self, db_path: Optional[str] = None):
        """
        Initialize the fragment manager

        Args:
            db_path: Path to SQLite database (default from settings)
        """
        self.db_path = db_path or settings.data_path / "fragments.db"
        self._initialized = False

    async def initialize(self) -> None:
        """Create database tables if they don't exist"""
        if self._initialized:
            return

        # Ensure data directory exists
        Path(self.db_path).parent.mkdir(parents=True, exist_ok=True)

        async with aiosqlite.connect(self.db_path) as db:
            await db.executescript("""
                CREATE TABLE IF NOT EXISTS sessions (
                    id TEXT PRIMARY KEY,
                    name TEXT DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    total_duration REAL DEFAULT 0,
                    fragment_count INTEGER DEFAULT 0,
                    status TEXT DEFAULT 'active'
                );

                CREATE TABLE IF NOT EXISTS fragments (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    audio_path TEXT DEFAULT '',
                    raw_text TEXT DEFAULT '',
                    optimized_text TEXT DEFAULT '',
                    start_time REAL DEFAULT 0,
                    end_time REAL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    status TEXT DEFAULT 'pending',
                    metadata TEXT DEFAULT '{}',
                    FOREIGN KEY (session_id) REFERENCES sessions(id)
                );

                CREATE INDEX IF NOT EXISTS idx_fragments_session ON fragments(session_id);
                CREATE INDEX IF NOT EXISTS idx_fragments_status ON fragments(status);
            """)
            await db.commit()

        self._initialized = True
        logger.info("Database initialized", db_path=str(self.db_path))

    async def create_session(
        self,
        name: str = "",
        session_id: Optional[str] = None
    ) -> Session:
        """
        Create a new session

        Args:
            name: Session name
            session_id: Optional custom session ID

        Returns:
            Created Session object
        """
        await self.initialize()

        import uuid
        session = Session(
            id=session_id or str(uuid.uuid4()),
            name=name,
            created_at=datetime.now(),
            updated_at=datetime.now()
        )

        async with aiosqlite.connect(self.db_path) as db:
            await db.execute(
                """INSERT INTO sessions
                   (id, name, created_at, updated_at, status)
                   VALUES (?, ?, ?, ?, ?)""",
                (
                    session.id,
                    session.name,
                    session.created_at.isoformat(),
                    session.updated_at.isoformat(),
                    session.status
                )
            )
            await db.commit()

        logger.info("Session created", session_id=session.id, name=name)
        return session

    async def get_session(self, session_id: str) -> Optional[Session]:
        """Get a session by ID"""
        await self.initialize()

        async with aiosqlite.connect(self.db_path) as db:
            async with db.execute(
                "SELECT * FROM sessions WHERE id = ?",
                (session_id,)
            ) as cursor:
                row = await cursor.fetchone()

                if row:
                    return Session(
                        id=row[0],
                        name=row[1],
                        created_at=datetime.fromisoformat(row[2]),
                        updated_at=datetime.fromisoformat(row[3]),
                        total_duration=row[4],
                        fragment_count=row[5],
                        status=row[6]
                    )
        return None

    async def list_sessions(
        self,
        status: Optional[str] = None,
        limit: int = 100
    ) -> List[Session]:
        """List all sessions"""
        await self.initialize()

        query = "SELECT * FROM sessions"
        params = []

        if status:
            query += " WHERE status = ?"
            params.append(status)

        query += " ORDER BY created_at DESC LIMIT ?"
        params.append(limit)

        async with aiosqlite.connect(self.db_path) as db:
            async with db.execute(query, params) as cursor:
                rows = await cursor.fetchall()

                return [
                    Session(
                        id=row[0],
                        name=row[1],
                        created_at=datetime.fromisoformat(row[2]),
                        updated_at=datetime.fromisoformat(row[3]),
                        total_duration=row[4],
                        fragment_count=row[5],
                        status=row[6]
                    )
                    for row in rows
                ]

    async def create_fragment(
        self,
        session_id: str,
        audio_path: str = "",
        raw_text: str = "",
        start_time: float = 0,
        end_time: float = 0,
        metadata: Optional[dict] = None
    ) -> Fragment:
        """
        Create a new fragment

        Args:
            session_id: Session this fragment belongs to
            audio_path: Path to audio file
            raw_text: Raw transcription text
            start_time: Start time in seconds
            end_time: End time in seconds
            metadata: Additional metadata

        Returns:
            Created Fragment object
        """
        await self.initialize()

        fragment = Fragment(
            session_id=session_id,
            audio_path=audio_path,
            raw_text=raw_text,
            start_time=start_time,
            end_time=end_time,
            metadata=metadata or {}
        )

        async with aiosqlite.connect(self.db_path) as db:
            await db.execute(
                """INSERT INTO fragments
                   (id, session_id, audio_path, raw_text, optimized_text,
                    start_time, end_time, created_at, updated_at, status, metadata)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)""",
                (
                    fragment.id,
                    fragment.session_id,
                    fragment.audio_path,
                    fragment.raw_text,
                    fragment.optimized_text,
                    fragment.start_time,
                    fragment.end_time,
                    fragment.created_at.isoformat(),
                    fragment.updated_at.isoformat(),
                    fragment.status.value,
                    json.dumps(fragment.metadata)
                )
            )

            # Update session fragment count
            await db.execute(
                """UPDATE sessions
                   SET fragment_count = fragment_count + 1,
                       updated_at = ?,
                       total_duration = total_duration + ?
                   WHERE id = ?""",
                (datetime.now().isoformat(), end_time - start_time, session_id)
            )

            await db.commit()

        logger.info(
            "Fragment created",
            fragment_id=fragment.id,
            session_id=session_id
        )
        return fragment

    async def get_fragment(self, fragment_id: str) -> Optional[Fragment]:
        """Get a fragment by ID"""
        await self.initialize()

        async with aiosqlite.connect(self.db_path) as db:
            async with db.execute(
                "SELECT * FROM fragments WHERE id = ?",
                (fragment_id,)
            ) as cursor:
                row = await cursor.fetchone()

                if row:
                    return Fragment(
                        id=row[0],
                        session_id=row[1],
                        audio_path=row[2],
                        raw_text=row[3],
                        optimized_text=row[4],
                        start_time=row[5],
                        end_time=row[6],
                        created_at=datetime.fromisoformat(row[7]),
                        updated_at=datetime.fromisoformat(row[8]),
                        status=FragmentStatus(row[9]),
                        metadata=json.loads(row[10]) if row[10] else {}
                    )
        return None

    async def list_fragments(
        self,
        session_id: Optional[str] = None,
        status: Optional[FragmentStatus] = None,
        limit: int = 100
    ) -> List[Fragment]:
        """List fragments with optional filters"""
        await self.initialize()

        query = "SELECT * FROM fragments"
        params = []
        conditions = []

        if session_id:
            conditions.append("session_id = ?")
            params.append(session_id)

        if status:
            conditions.append("status = ?")
            params.append(status.value)

        if conditions:
            query += " WHERE " + " AND ".join(conditions)

        query += " ORDER BY start_time ASC LIMIT ?"
        params.append(limit)

        async with aiosqlite.connect(self.db_path) as db:
            async with db.execute(query, params) as cursor:
                rows = await cursor.fetchall()

                return [
                    Fragment(
                        id=row[0],
                        session_id=row[1],
                        audio_path=row[2],
                        raw_text=row[3],
                        optimized_text=row[4],
                        start_time=row[5],
                        end_time=row[6],
                        created_at=datetime.fromisoformat(row[7]),
                        updated_at=datetime.fromisoformat(row[8]),
                        status=FragmentStatus(row[9]),
                        metadata=json.loads(row[10]) if row[10] else {}
                    )
                    for row in rows
                ]

    async def update_fragment(
        self,
        fragment_id: str,
        raw_text: Optional[str] = None,
        optimized_text: Optional[str] = None,
        status: Optional[FragmentStatus] = None
    ) -> Optional[Fragment]:
        """Update a fragment"""
        await self.initialize()

        updates = []
        params = []

        if raw_text is not None:
            updates.append("raw_text = ?")
            params.append(raw_text)

        if optimized_text is not None:
            updates.append("optimized_text = ?")
            params.append(optimized_text)

        if status is not None:
            updates.append("status = ?")
            params.append(status.value)

        if not updates:
            return await self.get_fragment(fragment_id)

        updates.append("updated_at = ?")
        params.append(datetime.now().isoformat())

        params.append(fragment_id)

        async with aiosqlite.connect(self.db_path) as db:
            await db.execute(
                f"UPDATE fragments SET {', '.join(updates)} WHERE id = ?",
                params
            )
            await db.commit()

        return await self.get_fragment(fragment_id)

    async def merge_fragments(
        self,
        fragment_ids: List[str],
        use_optimized: bool = True
    ) -> str:
        """
        Merge multiple fragments into a single text

        Args:
            fragment_ids: List of fragment IDs to merge
            use_optimized: Use optimized text if available

        Returns:
            Merged text
        """
        fragments = []
        for fid in fragment_ids:
            fragment = await self.get_fragment(fid)
            if fragment:
                fragments.append(fragment)

        # Sort by start time
        fragments.sort(key=lambda f: f.start_time)

        lines = []
        for f in fragments:
            text = f.optimized_text if use_optimized and f.optimized_text else f.raw_text
            if text:
                lines.append(text)

        return "\n\n".join(lines)

    async def export_session(
        self,
        session_id: str,
        format: ExportFormat = ExportFormat.TXT,
        use_optimized: bool = True
    ) -> str:
        """
        Export a session's content

        Args:
            session_id: Session to export
            format: Export format (TXT, JSON, SRT)
            use_optimized: Use optimized text if available

        Returns:
            Exported content as string
        """
        fragments = await self.list_fragments(session_id=session_id, limit=10000)

        if format == ExportFormat.TXT:
            lines = []
            for f in fragments:
                text = f.optimized_text if use_optimized and f.optimized_text else f.raw_text
                if text:
                    # Add timestamp
                    start_min = int(f.start_time // 60)
                    start_sec = f.start_time % 60
                    lines.append(f"[{start_min:02d}:{start_sec:05.2f}] {text}")
            return "\n".join(lines)

        elif format == ExportFormat.JSON:
            import json
            return json.dumps(
                [f.to_dict() for f in fragments],
                ensure_ascii=False,
                indent=2
            )

        elif format == ExportFormat.SRT:
            lines = []
            for i, f in enumerate(fragments, 1):
                text = f.optimized_text if use_optimized and f.optimized_text else f.raw_text
                if text:
                    # SRT timestamp format
                    start_h = int(f.start_time // 3600)
                    start_m = int((f.start_time % 3600) // 60)
                    start_s = f.start_time % 60

                    end_h = int(f.end_time // 3600)
                    end_m = int((f.end_time % 3600) // 60)
                    end_s = f.end_time % 60

                    lines.append(str(i))
                    lines.append(
                        f"{start_h:02d}:{start_m:02d}:{start_s:06.3f} --> "
                        f"{end_h:02d}:{end_m:02d}:{end_s:06.3f}"
                    )
                    lines.append(text)
                    lines.append("")

            return "\n".join(lines)

        return ""

    async def delete_fragment(self, fragment_id: str) -> bool:
        """Delete a fragment"""
        await self.initialize()

        async with aiosqlite.connect(self.db_path) as db:
            # Get fragment info first
            fragment = await self.get_fragment(fragment_id)
            if not fragment:
                return False

            await db.execute(
                "DELETE FROM fragments WHERE id = ?",
                (fragment_id,)
            )

            # Update session
            await db.execute(
                """UPDATE sessions
                   SET fragment_count = fragment_count - 1,
                       updated_at = ?,
                       total_duration = total_duration - ?
                   WHERE id = ?""",
                (datetime.now().isoformat(), fragment.end_time - fragment.start_time, fragment.session_id)
            )

            await db.commit()

        logger.info("Fragment deleted", fragment_id=fragment_id)
        return True

    async def delete_session(self, session_id: str) -> bool:
        """Delete a session and all its fragments"""
        await self.initialize()

        async with aiosqlite.connect(self.db_path) as db:
            # Delete fragments first
            await db.execute(
                "DELETE FROM fragments WHERE session_id = ?",
                (session_id,)
            )

            # Delete session
            await db.execute(
                "DELETE FROM sessions WHERE id = ?",
                (session_id,)
            )

            await db.commit()

        logger.info("Session deleted", session_id=session_id)
        return True
