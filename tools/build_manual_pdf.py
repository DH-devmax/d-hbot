#!/usr/bin/env python3
"""Build the concise Chinese DH manual with annotated interface screenshots."""

from pathlib import Path
import sys

from reportlab.lib import colors
from reportlab.lib.pagesizes import A4, landscape
from reportlab.lib.styles import ParagraphStyle
from reportlab.lib.units import mm
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas
from reportlab.platypus import Paragraph
from reportlab.lib.utils import ImageReader


ROOT = Path(__file__).resolve().parents[1]
PREVIEW = ROOT / "docs" / "screenshots" / "dh270"
FONT_NAME = "DHManual"
FONT_CANDIDATES = [
    Path("/System/Library/Fonts/Supplemental/Arial Unicode.ttf"),
    Path("/Library/Fonts/Arial Unicode.ttf"),
]


PAGES = [
    ("总览", "overview.png", "先看连接、AI、启用群和最近自动动作。每日摘要只在 DH 总览显示，不发送到群里。", [(0.91, 0.86, "连接状态")]),
    ("群组与成员", "groups.png", "先同步群，再用群组和成员两个 TAB 切换。所有群名片操作都按当前选择的群执行。", [(0.40, 0.65, "群组/成员 TAB"), (0.52, 0.73, "同步和启用")]),
    ("消息台", "messages.png", "按群名称选择目标群，输入文本后发送。最近消息类型显示为文本、图片、名片或其他。", [(0.47, 0.76, "选择群")]),
    ("规则", "rules.png", "默认规则是全局规则。新增关键词时通常只勾选撤回；禁言、移出等动作需要管理员明确打开。", [(0.78, 0.72, "命中动作")]),
    ("知识与 AI", "knowledge.png", "先选择知识库，再绑定一个或多个群。没有绑定群的知识库不会进入 AI 请求。", [(0.36, 0.79, "选择知识库"), (0.53, 0.70, "绑定群")]),
    ("任务与摘要", "tasks.png", "任务用于保存待办；每日摘要只写入本机总览。下方计划可按电脑时区每日自动开群和关群。", [(0.82, 0.72, "创建任务"), (0.57, 0.58, "开群 / 关群")]),
    ("审计", "audit.png", "这里查看欢迎、规则、AI、任务和群管动作。出现问题时按时间查看最后几条记录。", [(0.45, 0.76, "筛选和事件")]),
    ("设置", "settings.png", "旺商聊路径、固定登录分区、DevTools、AI 地址和模型都在这里。Base URL 可填写根地址，DH 会自动补充 /v1。", [(0.74, 0.72, "登录分区"), (0.52, 0.66, "连接设置")]),
    ("AI 本地测试", "ai-test.png", "这个窗口只检查当前 AI 设置和人格。它不读取群消息，不发送群消息，模型返回的动作和任务也只显示。", [(0.25, 0.75, "输入问题"), (0.40, 0.45, "查看完整回复")]),
    ("调试", "debug.png", "先看 DevTools 和内嵌桥，再看协议会话/NIM 和固定登录分区。NIM 未就绪时显示旺商聊，在旺商聊窗口完成登录。", [(0.80, 0.52, "NIM 状态"), (0.80, 0.42, "固定分区")]),
]


def register_font():
    for candidate in FONT_CANDIDATES:
        if candidate.exists():
            pdfmetrics.registerFont(TTFont(FONT_NAME, str(candidate)))
            return
    raise FileNotFoundError("未找到可嵌入的 Unicode 中文字体")


def draw_arrow(c, x, y, tx, ty, label):
    c.setStrokeColor(colors.HexColor("#D64045"))
    c.setFillColor(colors.HexColor("#D64045"))
    c.setLineWidth(1.5)
    c.line(x, y, tx, ty)
    dx, dy = tx - x, ty - y
    length = max((dx * dx + dy * dy) ** 0.5, 1.0)
    ux, uy = dx / length, dy / length
    size = 6
    left = (tx - ux * size - uy * size / 2, ty - uy * size + ux * size / 2)
    right = (tx - ux * size + uy * size / 2, ty - uy * size - ux * size / 2)
    path = c.beginPath()
    path.moveTo(tx, ty)
    path.lineTo(*left)
    path.lineTo(*right)
    path.close()
    c.drawPath(path, fill=1, stroke=0)
    c.setFont(FONT_NAME, 9)
    c.drawString(x + 4, y + 4, label)


def draw_screenshot_page(c, title, image_path, note, arrows, page_number, total_pages):
    width, height = landscape(A4)
    c.setFillColor(colors.HexColor("#FFFFFF"))
    c.rect(0, 0, width, height, fill=1, stroke=0)
    c.setFillColor(colors.HexColor("#111111"))
    c.setFont(FONT_NAME, 18)
    c.drawString(18 * mm, height - 15 * mm, title)
    c.setFillColor(colors.HexColor("#777777"))
    c.setFont(FONT_NAME, 9)
    c.drawRightString(width - 18 * mm, height - 14.5 * mm, "DH BOT 3.0 Beta 1 使用手册")

    image = ImageReader(str(image_path))
    iw, ih = image.getSize()
    max_w, max_h = width - 36 * mm, height - 68 * mm
    scale = min(max_w / iw, max_h / ih)
    draw_w, draw_h = iw * scale, ih * scale
    x, y = (width - draw_w) / 2, 29 * mm
    c.setStrokeColor(colors.HexColor("#DDDDDD"))
    c.rect(x - 1, y - 1, draw_w + 2, draw_h + 2, fill=0, stroke=1)
    c.drawImage(image, x, y, width=draw_w, height=draw_h, preserveAspectRatio=True, mask="auto")
    for nx, ny, label in arrows:
        tx, ty = x + nx * draw_w, y + ny * draw_h
        draw_arrow(c, min(width - 35 * mm, tx + 70), min(height - 25 * mm, ty + 55), tx, ty, label)

    style = ParagraphStyle("note", fontName=FONT_NAME, fontSize=9.5, leading=13, textColor=colors.HexColor("#333333"))
    paragraph = Paragraph(note, style)
    paragraph.wrapOn(c, width - 36 * mm, 17 * mm)
    paragraph.drawOn(c, 18 * mm, 8 * mm)
    c.setFont(FONT_NAME, 8)
    c.setFillColor(colors.HexColor("#999999"))
    c.drawRightString(width - 18 * mm, 9 * mm, f"{title} · {page_number}/{total_pages}")
    c.showPage()


def main(output: str):
    output_path = Path(output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    register_font()
    c = canvas.Canvas(str(output_path), pagesize=landscape(A4))
    c.setTitle("DH BOT 3.0 Beta 1 大白话使用手册")
    c.setAuthor("DH BOT")
    c.setSubject("旺商聊 AI 群管理工具图解手册")
    width, height = landscape(A4)
    c.setFillColor(colors.HexColor("#111111"))
    c.setFont(FONT_NAME, 34)
    c.drawString(24 * mm, height - 45 * mm, "DH BOT 3.0 Beta 1")
    c.setFont(FONT_NAME, 20)
    c.drawString(24 * mm, height - 60 * mm, "大白话使用手册")
    c.setFont(FONT_NAME, 12)
    c.setFillColor(colors.HexColor("#666666"))
    c.drawString(24 * mm, height - 78 * mm, "群管理、AI、知识库、任务和旺商聊连接")
    c.setFont(FONT_NAME, 11)
    c.drawString(24 * mm, 30 * mm, "Windows：双击 DH-BOT.exe。旺商聊登录一次后，DH BOT 会复用固定登录分区。")
    c.setFont(FONT_NAME, 8)
    c.drawRightString(width - 18 * mm, 9 * mm, f"1/{len(PAGES) + 1}")
    c.showPage()

    total_pages = len(PAGES) + 1
    for page_number, (title, filename, note, arrows) in enumerate(PAGES, start=2):
        image_path = PREVIEW / filename
        if not image_path.exists():
            raise FileNotFoundError(f"缺少 DH BOT 3.0 Beta 1 当前版截图：{image_path}")
        draw_screenshot_page(c, title, image_path, note, arrows, page_number, total_pages)

    c.save()
    print(output_path)


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else str(ROOT / "dist" / "DH-Manual-ZH.pdf"))
