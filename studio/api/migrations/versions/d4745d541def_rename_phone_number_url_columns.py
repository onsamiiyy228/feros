"""rename rust_voice_url to voice_server_url, drop py_backend_url

Revision ID: d4745d541def
Revises: d4e5f6a7b8c9
Create Date: 2026-03-16 04:30:00.000000
"""

from collections.abc import Sequence

from alembic import op

# revision identifiers, used by Alembic.
revision: str = "d4745d541def"
down_revision: str | None = "d4e5f6a7b8c9"
branch_labels: str | Sequence[str] | None = None
depends_on: str | Sequence[str] | None = None


def upgrade() -> None:
    op.alter_column(
        "phone_numbers",
        "rust_voice_url",
        new_column_name="voice_server_url",
    )
    op.drop_column("phone_numbers", "py_backend_url")


def downgrade() -> None:
    import sqlalchemy as sa

    op.add_column(
        "phone_numbers",
        sa.Column("py_backend_url", sa.Text(), nullable=True),
    )
    op.alter_column(
        "phone_numbers",
        "voice_server_url",
        new_column_name="rust_voice_url",
    )
