"""Gunicorn 配置文件"""


def post_fork(server, worker):
    """在 worker 进程 fork 后重建数据库连接池"""
    from src.database.database import dispose_engines
    dispose_engines()
