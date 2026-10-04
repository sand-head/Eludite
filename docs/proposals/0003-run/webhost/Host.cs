// A Cassini-style WebForms host for the spike: System.Web's real pipeline in an ASP.NET application domain,
// fed by HttpListener. GET only, no POST body (SimpleWorkerRequest), enough to prove page compilation and rendering.
using System;
using System.IO;
using System.Net;
using System.Web;
using System.Web.Hosting;

public class Host : MarshalByRefObject
{
    public string Process(string page, string query)
    {
        var sw = new StringWriter();
        HttpRuntime.ProcessRequest(new SimpleWorkerRequest(page, query, sw));
        return sw.ToString();
    }
    public override object InitializeLifetimeService() { return null; }

    public static int Main(string[] args)
    {
        string physical = Path.GetFullPath(args[0]);
        string prefix = args.Length > 1 ? args[1] : "http://127.0.0.1:8081/";
        var host = (Host)ApplicationHost.CreateApplicationHost(typeof(Host), "/", physical);
        var listener = new HttpListener();
        listener.Prefixes.Add(prefix);
        listener.Start();
        Console.WriteLine("listening " + prefix + " for " + physical);
        Console.Out.Flush();
        while (true)
        {
            var ctx = listener.GetContext();
            string page = ctx.Request.Url.AbsolutePath.TrimStart('/');
            if (page == "") page = "Default.aspx";
            if (page == "quit") { ctx.Response.Close(); break; }
            string body;
            int status = 200;
            try { body = host.Process(page, ctx.Request.Url.Query.TrimStart('?')); }
            catch (Exception e) { body = e.ToString(); status = 500; }
            var bytes = System.Text.Encoding.UTF8.GetBytes(body);
            ctx.Response.StatusCode = status;
            ctx.Response.ContentType = "text/html";
            ctx.Response.OutputStream.Write(bytes, 0, bytes.Length);
            ctx.Response.Close();
        }
        return 0;
    }
}
