using System.Globalization;
using System.IO;
using System.Text;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace Eludite.Debugger.Mono.Protocol;

/// <summary>
/// DAP's base protocol: each message is <c>Content-Length: N\r\n\r\n</c> followed by N bytes of UTF-8 JSON, as LSP
/// frames its messages. Other header fields are allowed and ignored.
/// </summary>
public static class Framing
{
    private static readonly UTF8Encoding Utf8 = new(encoderShouldEmitUTF8Identifier: false);

    /// <summary>The bytes of <paramref name="message"/> with its header.</summary>
    public static byte[] Encode(JObject message)
    {
        var body = Utf8.GetBytes(message.ToString(Formatting.None));
        var header = Encoding.ASCII.GetBytes("Content-Length: " + body.Length.ToString(CultureInfo.InvariantCulture) + "\r\n\r\n");
        var all = new byte[header.Length + body.Length];
        header.CopyTo(all, 0);
        body.CopyTo(all, header.Length);
        return all;
    }
}

/// <summary>Reads framed messages from a stream. Not thread-safe: one reader thread owns it.</summary>
public sealed class FrameReader
{
    private const int MaxHeaderLength = 8192;
    private readonly Stream _stream;
    private readonly byte[] _buffer = new byte[64 * 1024];
    private int _start;
    private int _end;

    public FrameReader(Stream stream)
    {
        _stream = stream;
    }

    /// <summary>The next message, or null at the end of the stream.</summary>
    /// <exception cref="InvalidDataException">The header or the JSON is malformed.</exception>
    public JObject? Read()
    {
        int? length = null;
        while (true)
        {
            var line = ReadHeaderLine();
            if (line is null)
            {
                return null;
            }

            if (line.Length == 0)
            {
                if (length is null)
                {
                    throw new InvalidDataException("a DAP message without Content-Length");
                }

                break;
            }

            var colon = line.IndexOf(':');
            if (colon > 0 && string.Equals(line.Substring(0, colon).Trim(), "Content-Length", StringComparison.OrdinalIgnoreCase))
            {
                if (!int.TryParse(line.Substring(colon + 1).Trim(), NumberStyles.None, CultureInfo.InvariantCulture, out var n) || n < 0)
                {
                    throw new InvalidDataException("a bad Content-Length: " + line);
                }

                length = n;
            }
        }

        var body = new byte[length.Value];
        var got = 0;
        while (got < body.Length)
        {
            if (_start == _end && !Fill())
            {
                return null;
            }

            var n = Math.Min(_end - _start, body.Length - got);
            Buffer.BlockCopy(_buffer, _start, body, got, n);
            _start += n;
            got += n;
        }

        var text = Encoding.UTF8.GetString(body);
        try
        {
            return JObject.Parse(text);
        }
        catch (JsonException e)
        {
            throw new InvalidDataException("a DAP message that is not a JSON object: " + e.Message, e);
        }
    }

    private bool Fill()
    {
        if (_start > 0 && _start == _end)
        {
            _start = _end = 0;
        }

        if (_end == _buffer.Length)
        {
            Buffer.BlockCopy(_buffer, _start, _buffer, 0, _end - _start);
            _end -= _start;
            _start = 0;
        }

        var n = _stream.Read(_buffer, _end, _buffer.Length - _end);
        if (n <= 0)
        {
            return false;
        }

        _end += n;
        return true;
    }

    /// <summary>One header line without its CRLF, or null at the end of the stream.</summary>
    private string? ReadHeaderLine()
    {
        var sb = new StringBuilder();
        while (true)
        {
            if (_start == _end && !Fill())
            {
                return null;
            }

            var b = _buffer[_start++];
            if (b == '\n')
            {
                if (sb.Length > 0 && sb[sb.Length - 1] == '\r')
                {
                    sb.Length--;
                }

                return sb.ToString();
            }

            sb.Append((char)b);
            if (sb.Length > MaxHeaderLength)
            {
                throw new InvalidDataException("a DAP header line longer than " + MaxHeaderLength.ToString(CultureInfo.InvariantCulture) + " bytes");
            }
        }
    }
}
