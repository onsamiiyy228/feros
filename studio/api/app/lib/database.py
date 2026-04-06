from collections.abc import AsyncGenerator

from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.orm import DeclarativeBase

from app.lib import get_settings

settings = get_settings()

# Normalize the URL to use the asyncpg driver. Users can specify a plain
# postgresql:// URL in .env.local — we handle the driver suffix here.
_db_url = settings.database.url
if _db_url.startswith("postgresql://"):
    _db_url = _db_url.replace("postgresql://", "postgresql+asyncpg://", 1)

engine = create_async_engine(
    _db_url,
    echo=settings.database.echo,
    pool_pre_ping=True,
    pool_size=settings.database.pool_size,
    max_overflow=settings.database.max_overflow,
    # Asyncpg attempts to cache prepared statements by default.
    # When connecting to a PgBouncer connection pooler in transaction mode (like Supabase port 6543),
    # prepared statement caching MUST be disabled to prevent statement mismatch errors.
    connect_args={
        "prepared_statement_cache_size": 0,
        "statement_cache_size": 0,
    },
)

async_session = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)


class Base(DeclarativeBase):
    """Base class for all SQLAlchemy models."""

    pass


async def get_db() -> AsyncGenerator[AsyncSession, None]:
    """FastAPI dependency that yields a database session."""
    async with async_session() as session:
        try:
            yield session
            await session.commit()
        except Exception:
            await session.rollback()
            raise
        finally:
            await session.close()
