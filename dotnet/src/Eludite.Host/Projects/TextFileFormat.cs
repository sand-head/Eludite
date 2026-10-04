using System.Text;

namespace Eludite.Host.Projects;

/// <summary>
/// How a text file is laid out on disk (brief 0049): its byte order mark, encoding, line endings, trailing newlines and
/// XML declaration, so that a file rewritten from a parsed model keeps every byte the edit did not change. XML and JSON
/// parsers normalize line endings to <c>\n</c> and some serializers rewrite the declaration; <see cref="Encode"/> puts
/// the file's own back.
/// </summary>
public sealed class TextFileFormat
{
    private TextFileFormat(byte[] preamble, Encoding encoding, string newLine, int trailingNewlines, string? declaration)
    {
        Preamble = preamble;
        Encoding = encoding;
        NewLine = newLine;
        TrailingNewlines = trailingNewlines;
        Declaration = declaration;
    }

    public byte[] Preamble { get; }

    public Encoding Encoding { get; }

    /// <summary><c>\r\n</c> when most line breaks are, else <c>\n</c>.</summary>
    public string NewLine { get; }

    public int TrailingNewlines { get; }

    /// <summary>The XML declaration as written (<c>&lt;?xml version="1.0" encoding="utf-8"?&gt;</c>), or null.</summary>
    public string? Declaration { get; }

    /// <summary>Reads the layout of <paramref name="bytes"/> and decodes them.</summary>
    public static TextFileFormat Detect(byte[] bytes, out string text)
    {
        ArgumentNullException.ThrowIfNull(bytes);
        byte[] preamble = [];
        Encoding encoding = new UTF8Encoding(false);
        if (bytes is [0xEF, 0xBB, 0xBF, ..])
        {
            preamble = [0xEF, 0xBB, 0xBF];
        }
        else if (bytes is [0xFF, 0xFE, ..])
        {
            preamble = [0xFF, 0xFE];
            encoding = new UnicodeEncoding(false, false);
        }
        else if (bytes is [0xFE, 0xFF, ..])
        {
            preamble = [0xFE, 0xFF];
            encoding = new UnicodeEncoding(true, false);
        }

        text = encoding.GetString(bytes, preamble.Length, bytes.Length - preamble.Length);
        var crlf = 0;
        var lf = 0;
        for (var i = 0; i < text.Length; i++)
        {
            if (text[i] == '\n')
            {
                if (i > 0 && text[i - 1] == '\r')
                {
                    crlf++;
                }
                else
                {
                    lf++;
                }
            }
        }

        var newLine = crlf > lf ? "\r\n" : "\n";
        var trailing = 0;
        var end = text.Length;
        while (end > 0)
        {
            if (text[end - 1] == '\n')
            {
                trailing++;
                end -= end > 1 && text[end - 2] == '\r' ? 2 : 1;
            }
            else
            {
                break;
            }
        }

        string? declaration = null;
        if (text.StartsWith("<?xml", StringComparison.Ordinal) && text.IndexOf("?>", StringComparison.Ordinal) is var close and > 0)
        {
            declaration = text[..(close + 2)];
        }

        return new TextFileFormat(preamble, encoding, newLine, trailing, declaration);
    }

    /// <summary>
    /// <paramref name="serialized"/> laid out as the original file: its declaration, line endings, trailing newlines,
    /// encoding and byte order mark.
    /// </summary>
    public byte[] Encode(string serialized)
    {
        ArgumentNullException.ThrowIfNull(serialized);
        var text = serialized.Replace("\r\n", "\n", StringComparison.Ordinal);
        if (text.StartsWith("<?xml", StringComparison.Ordinal) && text.IndexOf("?>", StringComparison.Ordinal) is var close and > 0)
        {
            text = Declaration is null ? text[(close + 2)..].TrimStart('\n') : Declaration.Replace("\r\n", "\n", StringComparison.Ordinal) + text[(close + 2)..];
        }

        text = text.TrimEnd('\n') + new string('\n', TrailingNewlines);
        if (NewLine == "\r\n")
        {
            text = text.Replace("\n", "\r\n", StringComparison.Ordinal);
        }

        var body = Encoding.GetBytes(text);
        return [.. Preamble, .. body];
    }

    /// <summary>Writes <paramref name="bytes"/> to <paramref name="path"/> through a temporary file beside it.</summary>
    public static void WriteAtomically(string path, byte[] bytes)
    {
        var temp = Path.Combine(Path.GetDirectoryName(path)!, $".{Path.GetFileName(path)}.eludite-{Environment.ProcessId}.tmp");
        File.WriteAllBytes(temp, bytes);
        File.Move(temp, path, overwrite: true);
    }
}
